// ShSnapshotPreview.test.ts — component tests for the read-only
// version-history preview mini-grid (#2339 PR 15b-beta).

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import ShSnapshotPreview from './ShSnapshotPreview.svelte';

function envelope(overrides: Record<string, unknown> = {}) {
  return JSON.stringify({
    v: 1,
    columns: [
      { id: 'col-a', name: 'Line item', col_type: 'text', options_json: '{}', width: 164, sort_order: 0 },
      { id: 'col-b', name: 'Total', col_type: 'currency', options_json: '{}', width: 118, sort_order: 1 },
    ],
    rows: [{ id: 'row-1', sort_order: 0, height: 32 }],
    cells: [
      { row_id: 'row-1', column_id: 'col-a', value: 'Longform essays', style_json: '{}' },
      { row_id: 'row-1', column_id: 'col-b', value: '=B1*2', style_json: '{}' },
    ],
    ...overrides,
  });
}

describe('ShSnapshotPreview', () => {
  it('renders a header per column with a type glyph and name', () => {
    render(ShSnapshotPreview, { props: { content: envelope() } });
    const headers = screen.getAllByTestId('sheet-snapshot-preview-header');
    expect(headers).toHaveLength(2);
    expect(headers[0].textContent).toContain('Line item');
    expect(headers[1].textContent).toContain('Total');
  });

  it('renders a data cell per column per row with its raw value', () => {
    render(ShSnapshotPreview, { props: { content: envelope() } });
    expect(screen.getByTestId('sheet-snapshot-preview-cell-0-0').textContent).toBe('Longform essays');
  });

  it('renders the computed value for a formula cell when a matching computed entry is provided (item 32, #2874)', () => {
    // col-b is a currency column, so per item 38 (#2874) its computed value
    // is rendered through the same shared currency formatter as any other
    // currency cell — '84' -> '$84.00', not the bare number.
    render(ShSnapshotPreview, {
      props: { content: envelope(), computedValues: { 'col-b:row-1': '84' } },
    });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).toBe('$84.00');
  });

  it('renders EMPTY (never the raw formula string) for a formula cell with no matching computed entry (item 32, #2874)', () => {
    render(ShSnapshotPreview, { props: { content: envelope(), computedValues: {} } });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).toBe('');
    expect(cell.textContent).not.toContain('=B1*2');
  });

  it('renders EMPTY when computedValues prop is entirely omitted (default-prop fallback)', () => {
    render(ShSnapshotPreview, { props: { content: envelope() } });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).toBe('');
  });

  it('renders an error string verbatim for a formula cell whose computed value is an error', () => {
    render(ShSnapshotPreview, {
      props: { content: envelope(), computedValues: { 'col-b:row-1': '#DIV/0!' } },
    });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).toBe('#DIV/0!');
  });

  it('never renders a raw internal column/row ID for a formula cell, even with computedValues absent', () => {
    render(ShSnapshotPreview, { props: { content: envelope(), computedValues: {} } });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).not.toMatch(/col-[a-z]|row-\d/);
  });

  it('does not mark a formula cell with any formula-wedge/calc class (#2874 item 55 — no formula indicators anywhere)', () => {
    // Regression guard: item 55 removed the cyan `.sh-prev__c--calc`
    // wedge that used to live here (ported originally from the live
    // grid's `.sh-w--calc`). Matt's ruling: "no formula indicators
    // anywhere" — a formula cell is visually indistinguishable from a
    // plain-value cell except for its rendered content.
    render(ShSnapshotPreview, { props: { content: envelope() } });
    const calcCell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    const plainCell = screen.getByTestId('sheet-snapshot-preview-cell-0-0');
    expect(calcCell.className).not.toContain('calc');
    expect(plainCell.className).not.toContain('calc');
  });

  it('applies preserved cell styling (bg/fg) as inline styles', () => {
    const withStyle = envelope({
      cells: [
        { row_id: 'row-1', column_id: 'col-a', value: 'Paid amplification', style_json: '{"bg":"rgba(239,68,68,0.13)"}' },
        { row_id: 'row-1', column_id: 'col-b', value: '5000', style_json: '{}' },
      ],
    });
    render(ShSnapshotPreview, { props: { content: withStyle } });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-0');
    expect(cell.getAttribute('style')).toContain('rgba(239, 68, 68, 0.13)');
  });

  it('renders a user-set background on a formula cell (#2874 item 55 — "users add borders or backgrounds if they want them" must still work)', () => {
    // The removed tint/wedge were both class-selector CSS; a user's own
    // background has always been emitted as an inline `style="background"`
    // declaration (cellInlineStyle()), which wins over any class rule by
    // CSS precedence regardless — so removing the formula-indicator CSS
    // cannot have regressed this path. Confirmed directly here: col-b is
    // the formula cell (`=B1*2`) and still renders its own user-set bg.
    const withStyle = envelope({
      cells: [
        { row_id: 'row-1', column_id: 'col-a', value: 'Longform essays', style_json: '{}' },
        { row_id: 'row-1', column_id: 'col-b', value: '=B1*2', style_json: '{"bg":"rgba(34,197,94,0.2)"}' },
      ],
    });
    render(ShSnapshotPreview, {
      props: { content: withStyle, computedValues: { 'col-b:row-1': '84' } },
    });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.getAttribute('style')).toContain('rgba(34, 197, 94, 0.2)');
    // Computed value still renders on top of the user background — the
    // visual-indicator removal doesn't touch value rendering.
    expect(cell.textContent).toBe('$84.00');
  });

  it('renders an "unreadable" fallback for malformed content, never throwing', () => {
    expect(() => render(ShSnapshotPreview, { props: { content: '{not json' } })).not.toThrow();
    render(ShSnapshotPreview, { props: { content: '{not json' } });
    expect(screen.getAllByTestId('sheet-snapshot-preview-unreadable').length).toBeGreaterThan(0);
    expect(screen.queryAllByTestId('sheet-snapshot-preview-header')).toHaveLength(0);
  });

  // #2874 item 38 (Matt's ruling 2026-09-15, ruling 3): the preview uses
  // the SAME shared `formatCurrencyValue()` formatter as the live grid —
  // no duplicated currency-formatting logic.
  it('ShSnapshotPreview currency cell uses the same format', () => {
    const withCurrency = envelope({
      columns: [
        { id: 'col-a', name: 'Line item', col_type: 'text', options_json: '{}', width: 164, sort_order: 0 },
        { id: 'col-b', name: 'Total', col_type: 'currency', options_json: '{"currency":"USD"}', width: 118, sort_order: 1 },
      ],
      cells: [
        { row_id: 'row-1', column_id: 'col-a', value: 'Longform essays', style_json: '{}' },
        { row_id: 'row-1', column_id: 'col-b', value: '930', style_json: '{}' },
      ],
    });
    render(ShSnapshotPreview, { props: { content: withCurrency } });
    const cell = screen.getByTestId('sheet-snapshot-preview-cell-0-1');
    expect(cell.textContent).toBe('$930.00');
  });
});
