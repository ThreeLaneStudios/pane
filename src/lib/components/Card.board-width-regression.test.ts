/**
 * #2339 PR 15a — Board card face: board column / card WIDTH regression test.
 *
 * Build constraint §1: "Do not change board column or card widths." The
 * sheet card face is a HEIGHT-ONLY revision — .pane-card's width, padding,
 * border-radius, and border must be byte-identical to pre-PR-15a for every
 * card (sheet or not), and .kanban-column's width rules must be completely
 * untouched by this PR.
 *
 * These are source-text-extraction / regex assertions against the real
 * component source, matching the house pattern established by
 * KanbanBoard.new-sheet-button-polish.test.ts (heavy Tauri/store dependency
 * components use this pattern rather than full mounts).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const CARD_PATH = resolve(__dirname, 'Card.svelte');
const CARD_SOURCE = readFileSync(CARD_PATH, 'utf-8');

const KANBAN_PATH = resolve(__dirname, 'KanbanBoard.svelte');
const KANBAN_SOURCE = readFileSync(KANBAN_PATH, 'utf-8');

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

describe('#2339 PR 15a — board card face is HEIGHT ONLY (no width/padding/radius drift)', () => {
  it('.pane-card base rule keeps its original padding, border-radius, and border — untouched by this PR', () => {
    const rule = extractCssRule(CARD_SOURCE, '.pane-card');
    expect(rule).toMatch(/padding:\s*10px 10px 8px 10px;/);
    expect(rule).toMatch(/border-radius:\s*4px;/);
    expect(rule).toMatch(/border:\s*1px solid rgba\(255,255,255,0\.06\);/);
    expect(rule).toMatch(/border-left:\s*3px solid transparent;/);
    // No width property at all on the base rule — width is 100% inherited
    // from whatever flex/grid container places the card (kanban column,
    // notes-browser list, all-tasks column). This PR must not introduce one.
    expect(rule).not.toMatch(/\bwidth:/);
  });

  it('.pane-card has no max-width/min-width rule anywhere in Card.svelte (this PR does not add one)', () => {
    expect(CARD_SOURCE).not.toMatch(/\.pane-card[^{]*\{[^}]*\bmax-width:/);
    expect(CARD_SOURCE).not.toMatch(/\.pane-card[^{]*\{[^}]*\bmin-width:/);
  });

  it('the sheet-card height override rule touches ONLY height/min-height/max-height — no width, padding, radius, or border properties', () => {
    const rule = extractCssRule(CARD_SOURCE, '.pane-card.sheet-card');
    expect(rule).toMatch(/height:\s*auto;/);
    expect(rule).not.toMatch(/\bwidth:/);
    expect(rule).not.toMatch(/\bpadding:/);
    expect(rule).not.toMatch(/border-radius/);
    expect(rule).not.toMatch(/\bborder:/);
  });

  it('KanbanBoard.svelte: .kanban-column min-width is untouched at 280px', () => {
    const rule = extractCssRule(KANBAN_SOURCE, '.kanban-column');
    expect(rule).toMatch(/min-width:\s*280px;/);
  });

  it('KanbanBoard.svelte: .kanban-columns gap/padding are untouched by this PR', () => {
    const rule = extractCssRule(KANBAN_SOURCE, '.kanban-columns');
    expect(rule).toMatch(/gap:\s*24px;/);
    expect(rule).toMatch(/padding:\s*24px;/);
  });

  it('KanbanBoard.svelte has no new .kanban-column/.pane-card width override introduced for sheets', () => {
    // Scoped guard (not a blanket "sheet" grep — the pre-existing sheet
    // EDITOR PANEL width state (kanbanSheetWidth, PR9 ruling 2) is legitimate
    // and unrelated to board card/column width): specifically, no CSS rule
    // combining .kanban-column or .pane-card with "sheet" in its selector
    // exists anywhere in this file.
    const sheetColumnOrCardWidthPattern = /\.(kanban-column|pane-card)[a-zA-Z0-9_.-]*sheet[a-zA-Z0-9_-]*\s*\{[^}]*\bwidth:/i;
    expect(KANBAN_SOURCE).not.toMatch(sheetColumnOrCardWidthPattern);
  });
});
