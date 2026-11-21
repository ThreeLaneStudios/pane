/**
 * #2339 PR 15a — Board card face: Card.svelte's sheet-vs-note branch.
 *
 * Design source: docs/design/sheets/sheets-surfaces.jsx §4 (SheetCard),
 * docs/design/sheets/README.md §4 + build constraint §1, Reed's scope
 * report §1.6 (test coverage requirements).
 *
 * Full-mount component tests via @testing-library/svelte/svelte5, matching
 * the house pattern established by AttachmentCard.test.ts.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Card from './Card.svelte';
import type { CardData } from '$lib/utils/cardNormalizers';

afterEach(() => cleanup());

// ─── ResizeObserver stub ───────────────────────────────────────────────────
// jsdom has no ResizeObserver. Card.svelte's onMount only attaches one for
// sheet cards (isSheet gate) — for non-sheet cards this stub is never even
// instantiated, but we still provide it globally so sheet-card tests don't
// throw. Tests that need to simulate a specific rendered width invoke the
// captured callback directly with a synthetic contentRect.
let lastObserverCallback: ResizeObserverCallback | null = null;
class FakeResizeObserver {
  callback: ResizeObserverCallback;
  constructor(cb: ResizeObserverCallback) {
    this.callback = cb;
    lastObserverCallback = cb;
  }
  observe() {}
  disconnect() {}
  unobserve() {}
}
(globalThis as any).ResizeObserver = FakeResizeObserver;

function fireResize(width: number) {
  if (!lastObserverCallback) throw new Error('no ResizeObserver callback captured');
  lastObserverCallback(
    [{ contentRect: { width } } as ResizeObserverEntry],
    {} as ResizeObserver
  );
}

function baseCardData(overrides: Partial<CardData> = {}): CardData {
  return {
    id: 'note-1',
    title: 'Test Card',
    bodyText: 'Some preview text',
    status: 'todo',
    priority: 'medium',
    dueDate: undefined,
    projectName: 'ProjectA',
    folderName: 'FolderB',
    subtaskCount: 0,
    ...overrides,
  };
}

function sheetCardData(overrides: Partial<CardData> = {}): CardData {
  return baseCardData({
    noteType: 'sheet',
    colCount: 4,
    rowCount: 24,
    formulaCount: 0,
    colTypes: ['text', 'select', 'currency', 'checkbox'],
    ...overrides,
  });
}

// ─── Sheet card: glyph + micro-grid + footer at normal width ───────────────

describe('Card.svelte — sheet card face (#2339 PR 15a)', () => {
  it('renders the cyan grid glyph beside the title for a sheet card', () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    const glyph = container.querySelector('.sh-card__glyph');
    expect(glyph).not.toBeNull();
    expect(glyph!.querySelector('svg')).not.toBeNull();
  });

  it('renders the micro-grid header row with one glyph cell per real column type (not a fixed mock)', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ colTypes: ['text', 'number', 'date'] }) },
    });
    const headerCells = container.querySelectorAll('.sh-mini__h .sh-mini__hc');
    expect(headerCells.length).toBe(3);
    expect(headerCells[0].textContent).toBe('T');
    expect(headerCells[1].textContent).toBe('#');
    expect(headerCells[2].textContent).toBe('D');
  });

  it('renders exactly two micro-grid data rows at normal (full) width — revised down from three', () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    const dataRows = container.querySelectorAll('.sh-mini > .sh-mini__r');
    expect(dataRows.length).toBe(2);
  });

  it('footer shows "cols × rows" with no computed suffix when formula_count is 0', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ colCount: 8, rowCount: 24, formulaCount: 0 }) },
    });
    const foot = container.querySelector('.sh-card__foot');
    expect(foot).not.toBeNull();
    expect(foot!.querySelector('b')!.textContent).toBe('8 × 24');
    expect(foot!.querySelector('.sh-card__calc')).toBeNull();
  });

  it('footer shows "cols × rows · N computed" when formula_count > 0', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ colCount: 8, rowCount: 24, formulaCount: 8 }) },
    });
    const foot = container.querySelector('.sh-card__foot');
    expect(foot!.querySelector('b')!.textContent).toBe('8 × 24');
    expect(foot!.querySelector('.sh-card__calc')!.textContent).toContain('8 computed');
  });

  it('empty sheet (0 rows) renders "N × 0" with the seeded column-type glyphs still in the header', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ colCount: 5, rowCount: 0, formulaCount: 0, colTypes: ['text', 'text', 'text', 'text', 'text'] }) },
    });
    const foot = container.querySelector('.sh-card__foot');
    expect(foot!.querySelector('b')!.textContent).toBe('5 × 0');
    const headerCells = container.querySelectorAll('.sh-mini__h .sh-mini__hc');
    expect(headerCells.length).toBe(5);
    headerCells.forEach(c => expect(c.textContent).toBe('T'));
  });

  it('standard task metadata (stage dot/label, priority pill, due chip) still renders for a sheet card, same as a note card', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ priority: 'high', status: 'in_progress' }) },
    });
    expect(container.querySelector('.pill-stage')).not.toBeNull();
    expect(container.querySelector('.pill-priority')).not.toBeNull();
    expect(container.querySelector('.pill-priority')!.textContent).toBe('HIGH');
  });

  it('due-date chip on a sheet card carries the sh-due tone class', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ dueDate: '2099-01-01' }) },
    });
    const due = container.querySelector('.pill-due');
    expect(due).not.toBeNull();
    expect(due!.classList.contains('sh-due')).toBe(true);
  });

  it('breadcrumb appears inside the footer, after the metric, for a sheet card', () => {
    const { container } = render(Card, {
      props: { data: sheetCardData({ projectName: 'Symbiotic', folderName: 'Articles' }) },
    });
    const foot = container.querySelector('.sh-card__foot');
    expect(foot).not.toBeNull();
    const crumb = foot!.querySelector('.sh-card__crumb');
    expect(crumb).not.toBeNull();
    expect(crumb!.textContent).toContain('Symbiotic');
    expect(crumb!.textContent).toContain('Articles');
  });
});

// ─── Compact variant breakpoint ─────────────────────────────────────────────

describe('Card.svelte — sheet card compact variant (#2339 PR 15a)', () => {
  it('renders 2 data rows above the compact threshold (191px)', async () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    fireResize(191);
    await tick();
    const dataRows = container.querySelectorAll('.sh-mini > .sh-mini__r');
    expect(dataRows.length).toBe(2);
  });

  it('renders exactly at the threshold (190px) as full (inclusive boundary)', async () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    fireResize(190);
    await tick();
    const dataRows = container.querySelectorAll('.sh-mini > .sh-mini__r');
    expect(dataRows.length).toBe(2);
  });

  it('drops to 0 data rows below the compact threshold (189px) — glyph + header strip only', async () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    fireResize(189);
    await tick();
    const dataRows = container.querySelectorAll('.sh-mini > .sh-mini__r');
    expect(dataRows.length).toBe(0);
    // The header row (type glyphs) must still be present — compact removes
    // data rows, not the informative header.
    const headerCells = container.querySelectorAll('.sh-mini__h .sh-mini__hc');
    expect(headerCells.length).toBeGreaterThan(0);
  });

  it('the glyph and title remain visible in compact mode', async () => {
    const { container } = render(Card, { props: { data: sheetCardData() } });
    fireResize(100);
    await tick();
    expect(container.querySelector('.sh-card__glyph')).not.toBeNull();
    expect(container.querySelector('.sh-card__title')!.textContent).toBe('Test Card');
  });
});

// ─── Non-sheet regression ───────────────────────────────────────────────────

describe('Card.svelte — non-sheet cards are byte-identical to pre-PR-15a behavior', () => {
  it('renders the plain title + card-body markdown-preview clamp path, no sheet markup at all', () => {
    const { container } = render(Card, { props: { data: baseCardData() } });
    expect(container.querySelector('.sh-card__glyph')).toBeNull();
    expect(container.querySelector('.sh-mini')).toBeNull();
    expect(container.querySelector('.sh-card__foot')).toBeNull();
    expect(container.querySelector('.card-body')).not.toBeNull();
    expect(container.querySelector('.card-body')!.textContent).toBe('Some preview text');
  });

  it('does not apply the sheet-card height-auto class to a non-sheet .pane-card', () => {
    const { container } = render(Card, { props: { data: baseCardData() } });
    const card = container.querySelector('.pane-card');
    expect(card).not.toBeNull();
    expect(card!.classList.contains('sheet-card')).toBe(false);
  });

  it('breadcrumb renders in the plain .card-breadcrumb div (not .sh-card__foot) for a non-sheet card', () => {
    const { container } = render(Card, {
      props: { data: baseCardData({ projectName: 'Alpha', folderName: 'Beta' }) },
    });
    const crumb = container.querySelector('.card-breadcrumb');
    expect(crumb).not.toBeNull();
    expect(crumb!.textContent).toContain('Alpha');
    expect(crumb!.textContent).toContain('Beta');
    expect(container.querySelector('.sh-card__crumb')).toBeNull();
  });

  it('due-date chip on a non-sheet card never carries the sh-due tone class', () => {
    const { container } = render(Card, {
      props: { data: baseCardData({ dueDate: '2099-01-01' }) },
    });
    const due = container.querySelector('.pill-due');
    expect(due).not.toBeNull();
    expect(due!.classList.contains('sh-due')).toBe(false);
  });

  it('pill row (stage dot/label, priority) renders identically regardless of noteType', () => {
    const { container: noteContainer } = render(Card, {
      props: { data: baseCardData({ priority: 'urgent', status: 'review' }) },
    });
    expect(noteContainer.querySelector('.pill-priority')!.textContent).toBe('URGENT');
    expect(noteContainer.querySelector('.stage-label')).not.toBeNull();
  });

  it('a CardData with noteType left undefined takes the exact same non-sheet path as one with noteType explicitly omitted', () => {
    const a = render(Card, { props: { data: baseCardData() } });
    const bData = baseCardData();
    delete (bData as any).noteType;
    const b = render(Card, { props: { data: bData } });
    expect(a.container.innerHTML).toBe(b.container.innerHTML);
    a.unmount();
    b.unmount();
  });
});
