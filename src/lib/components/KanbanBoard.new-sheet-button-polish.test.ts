/**
 * #2339 PR 15c — New Sheet button polish (KanbanBoard.svelte)
 *
 * Replaces PR 9's minimal New Sheet button (grid glyph, static, no responsive
 * behavior) with the locked addendum design: plus glyph (reused from New Note,
 * not reimplemented), a new global `.btn--outline` CSS treatment, New Note
 * staying filled/rightmost, and a three-step responsive collapse keyed to the
 * HOST HEADER's own width via ResizeObserver.
 *
 * Design source: docs/design/sheets/sheets-addendum.jsx, NOTES.md §A,
 * README.md "Addendum rulings" #7.
 *
 * These tests exercise the REAL component source (source-text extraction /
 * regex assertions), matching the established house pattern for this file —
 * see KanbanBoard.folder-assignment.test.ts and
 * NotesBrowser.collapse-rails.test.ts's own docstrings on why full component
 * mounts aren't used here (heavy Tauri/store/context dependencies).
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import { resolveHeaderButtonMode } from '$lib/utils/newSheetButtonLayout';
import KanbanBoard from './KanbanBoard.svelte';
import * as api from '$lib/api';
import { canvasState } from '$lib/stores/canvas';

const COMPONENT_PATH = resolve(__dirname, 'KanbanBoard.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) lacking addEventListener/removeEventListener/ResizeObserver.
// KanbanBoard registers several window listeners and a ResizeObserver in
// onMount, so both need stubs for mount/unmount to succeed (same pattern as
// ChatSidebar's echoSuppression test suite / AllTasksView's polish tests).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

const APP_CSS_PATH = resolve(__dirname, '../../app.css');
const APP_CSS = readFileSync(APP_CSS_PATH, 'utf-8');

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

describe('KanbanBoard New Sheet button polish — #2339 PR 15c', () => {
  // ── Glyph swap ──────────────────────────────────────────────────────────

  it('New Sheet button carries the SAME plus glyph path as New Note, not the grid glyph', () => {
    // The exact plus-glyph markup shared verbatim by New Note.
    const plusGlyph = '<path d="M7 2V12M2 7H12" stroke-width="1.5" stroke-linecap="round"/>';
    const occurrences = SOURCE.split(plusGlyph).length - 1;
    // Appears at least twice: once for New Note, at least once for New Sheet
    // (full mode) — and again inside the caret menu's New Sheet row.
    expect(occurrences).toBeGreaterThanOrEqual(3);
  });

  it('the old grid-glyph SVG (rect + cross-hatch path) is gone from the New Sheet markup', () => {
    expect(SOURCE).not.toContain('rx="1.2" stroke-width="1.3"');
    expect(SOURCE).not.toContain('M1.5 5.2H12.5M1.5 8.8H12.5M5.2 1.5V12.5M8.8 1.5V12.5');
  });

  // ── .btn--outline, not .btn--ghost ─────────────────────────────────────

  it('New Sheet button applies the .btn--outline class', () => {
    expect(SOURCE).toMatch(/class="add-sheet-btn btn--outline"/);
  });

  it('never applies .btn--ghost anywhere in this component (explicitly rejected in the locked addendum)', () => {
    expect(SOURCE).not.toMatch(/btn--ghost/);
  });

  it('New Sheet is never given .btn--primary (that stays New Note-only, unstyled by this class in Kanban\'s own CSS)', () => {
    expect(SOURCE).not.toMatch(/add-sheet-btn[^"]*btn--primary/);
  });

  it('app.css defines .btn--outline as fill-only (transparent bg, accent border/text) — not .btn--ghost', () => {
    const rule = extractCssRule(APP_CSS, '.btn--outline');
    expect(rule).toMatch(/background:\s*transparent/);
    expect(rule).toMatch(/color:\s*var\(--accent\)/);
    expect(rule).toMatch(/border-color:/);
  });

  it('app.css .btn--outline:hover uses the accent-soft token, matching .btn--primary-adjacent semantics', () => {
    const rule = extractCssRule(APP_CSS, '.btn--outline:hover');
    expect(rule).toMatch(/background:\s*var\(--accent-soft\)/);
    expect(rule).toMatch(/border-color:\s*var\(--accent\)/);
  });

  // ── Positioning: New Note stays filled/rightmost ───────────────────────

  it('New Sheet button markup appears BEFORE New Note in DOM order (New Sheet sits to New Note\'s left)', () => {
    const sheetIndex = SOURCE.indexOf('class="add-sheet-btn btn--outline"');
    const noteIndex = SOURCE.indexOf('class="add-task-btn"');
    expect(sheetIndex).toBeGreaterThan(-1);
    expect(noteIndex).toBeGreaterThan(-1);
    expect(sheetIndex).toBeLessThan(noteIndex);
  });

  it('New Note (.add-task-btn) keeps its original filled/primary CSS treatment untouched', () => {
    const rule = extractCssRule(SOURCE, '.add-task-btn');
    expect(rule).toMatch(/background:\s*#00E4E4/);
  });

  it('New Note button unconditionally renders its label at every header width (never gated by headerButtonMode)', () => {
    const btnBlock = SOURCE.slice(
      SOURCE.indexOf('class="add-task-btn"'),
      SOURCE.indexOf('</button>', SOURCE.indexOf('class="add-task-btn"')),
    );
    expect(btnBlock).toContain('New Note');
    expect(btnBlock).not.toMatch(/{#if/);
  });

  // ── Three-step responsive collapse ─────────────────────────────────────

  it('imports resolveHeaderButtonMode from the shared pure-utility module (not reimplemented inline)', () => {
    expect(SOURCE).toMatch(/import\s+\{\s*resolveHeaderButtonMode,\s*type HeaderButtonMode\s*\}\s+from\s+'\$lib\/utils\/newSheetButtonLayout'/);
  });

  it('derives headerButtonMode reactively from kanbanHeaderWidth via $derived', () => {
    expect(SOURCE).toMatch(/let headerButtonMode: HeaderButtonMode = \$derived\(resolveHeaderButtonMode\(kanbanHeaderWidth\)\)/);
  });

  it('attaches a ResizeObserver to the header element in onMount and disconnects it in onDestroy', () => {
    expect(SOURCE).toMatch(/new ResizeObserver\(entries => \{/);
    expect(SOURCE).toMatch(/headerResizeObs\.observe\(kanbanHeaderEl\)/);
    expect(SOURCE).toMatch(/headerResizeObs\?\.disconnect\(\)/);
  });

  it('the .kanban-header element carries the bind:this ref the ResizeObserver observes', () => {
    expect(SOURCE).toMatch(/<div class="kanban-header" bind:this=\{kanbanHeaderEl\}>/);
  });

  it('New Sheet label is conditionally rendered only in "full" mode', () => {
    expect(SOURCE).toMatch(/\{#if headerButtonMode === 'full'\}New Sheet\{\/if\}/);
  });

  it('New Sheet button (full/icon modes) is hidden entirely once mode is "menu"', () => {
    expect(SOURCE).toMatch(/\{#if headerButtonMode !== 'menu'\}\s*<button\s*\n\s*class="add-sheet-btn btn--outline"/);
  });

  it('the icon-only CSS class toggles via class:add-sheet-btn--icon-only bound to headerButtonMode === \'icon\'', () => {
    expect(SOURCE).toMatch(/class:add-sheet-btn--icon-only=\{headerButtonMode === 'icon'\}/);
  });

  it('.add-sheet-btn--icon-only forces a 32px square box with no gap, matching New Note\'s height', () => {
    const rule = extractCssRule(SOURCE, '.add-sheet-btn--icon-only');
    expect(rule).toMatch(/width:\s*32px/);
    expect(rule).toMatch(/height:\s*32px/);
  });

  // ── Caret menu (header width < 720px) ──────────────────────────────────

  it('renders the caret button only when headerButtonMode === "menu"', () => {
    expect(SOURCE).toMatch(/\{#if headerButtonMode === 'menu'\}\s*<div class="sheet-caret-wrap">/);
  });

  it('the caret sits beside New Note (New Note renders immediately before the caret wrap in "menu" mode)', () => {
    const noteIndex = SOURCE.indexOf('class="add-task-btn"');
    const caretIndex = SOURCE.indexOf('class="sheet-caret-wrap"');
    expect(noteIndex).toBeGreaterThan(-1);
    expect(caretIndex).toBeGreaterThan(-1);
    expect(noteIndex).toBeLessThan(caretIndex);
  });

  it('handleNewSheetFromCaret closes the caret and delegates to the real handleCreateNewSheet (no duplicated creation logic)', () => {
    expect(SOURCE).toMatch(/function handleNewSheetFromCaret\(\) \{\s*sheetCaretOpen = false;\s*handleCreateNewSheet\(\);\s*\}/);
  });

  // ── Existing creation behavior preserved ────────────────────────────

  it('the full/icon-mode New Sheet button is still wired to handleCreateNewSheet (existing create-sheet behavior untouched)', () => {
    const btnStart = SOURCE.indexOf('class="add-sheet-btn btn--outline"');
    const btnBlockEnd = SOURCE.indexOf('</button>', btnStart);
    const btnBlock = SOURCE.slice(SOURCE.lastIndexOf('<button', btnStart), btnBlockEnd);
    expect(btnBlock).toMatch(/onclick=\{handleCreateNewSheet\}/);
    expect(btnBlock).toMatch(/disabled=\{creatingSheet\}/);
  });

  it('the caret\'s New Sheet row is ALSO wired through to handleCreateNewSheet (via handleNewSheetFromCaret, same code path)', () => {
    expect(SOURCE).toMatch(/onclick=\{handleNewSheetFromCaret\} disabled=\{creatingSheet\}/);
  });

  it('handleCreateNewSheet itself is untouched by this PR \u2014 still calls api.createSheet with the ambient project id', () => {
    expect(SOURCE).toMatch(/async function handleCreateNewSheet\(\) \{/);
    expect(SOURCE).toMatch(/await api\.createSheet\(\{/);
  });

  // ── resolveHeaderButtonMode integration sanity ─────────────────────

  it('the imported resolveHeaderButtonMode resolves the exact three modes Kanban\'s template branches on', () => {
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
    canvasState.set({
      view: 'kanban',
      data: {},
      context: { projectId: 'project-1', projectName: 'Project One' },
    });

    render(KanbanBoard, { props: {} });

    const caretButtons = Array.from(document.querySelectorAll('.sheet-caret-btn'));
    expect(caretButtons.length).toBeGreaterThan(0);
    await fireEvent.click(caretButtons[0] as HTMLElement);

    const items = Array.from(document.querySelectorAll('.sheet-caret-item'));
    expect(items.length).toBe(1);
    expect(items[0].textContent).toContain('New Sheet');
  });
});

