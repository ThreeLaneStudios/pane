// sheetCellRender.test.ts — unit tests for the Sheets cell-kind/display
// classification helpers (#2339 PR 10, §5.3/§7/§8). Every test drives the
// real `cellKind`/`displayValue`/`cellClass` functions against real
// SheetCell-shaped fixtures — no mocking.

import { describe, it, expect } from 'vitest';
import { cellKind, displayValue, cellClass, typeGlyph, cellInlineStyle, currencyCode, formatCurrencyValue } from './sheetCellRender';
import type { SheetCell } from '$lib/types';

describe('typeGlyph', () => {
  it('maps every known column type to its glyph', () => {
    expect(typeGlyph('text')).toBe('T');
    expect(typeGlyph('number')).toBe('#');
    expect(typeGlyph('currency')).toBe('$');
    expect(typeGlyph('date')).toBe('D');
    expect(typeGlyph('checkbox')).toBe('☑');
    expect(typeGlyph('select')).toBe('≡');
  });

  it('returns an empty string for an unrecognized type', () => {
    expect(typeGlyph('unknown')).toBe('');
  });
});

function cell(overrides: Partial<SheetCell> = {}): SheetCell {
  return {
    row_id: 'row-1',
    column_id: 'col-a',
    note_id: 'note-1',
    raw_value: overrides.raw_value ?? '',
    computed_value: overrides.computed_value,
    style_json: overrides.style_json ?? '{}',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

describe('cellKind', () => {
  it('returns "normal" for a null cell (empty coordinate)', () => {
    expect(cellKind(null, 'text')).toBe('normal');
  });

  it('returns "normal" for a plain non-formula value', () => {
    expect(cellKind(cell({ raw_value: 'hello' }), 'text')).toBe('normal');
  });

  it('returns "normal" for an empty string raw_value', () => {
    expect(cellKind(cell({ raw_value: '' }), 'text')).toBe('normal');
  });

  it('returns "calc" for a formula cell with a resolved, non-error computed_value', () => {
    expect(cellKind(cell({ raw_value: '=A1+B1', computed_value: '42' }), 'number')).toBe('calc');
  });

  it('returns "err" for a formula cell whose computed_value is an error string', () => {
    expect(cellKind(cell({ raw_value: '=1/0', computed_value: '#DIV/0!' }), 'number')).toBe('err');
  });

  // #2339 PR 10 §5.3 — the regression test this fix exists for: a formula
  // cell with NO computed_value must render as 'raw', never 'calc'.
  it('returns "raw" (not "calc") for a formula cell with null computed_value', () => {
    expect(cellKind(cell({ raw_value: '=A1+B1', computed_value: undefined }), 'number')).toBe('raw');
  });

  it('returns "raw" for a formula cell whose computed_value is explicitly null-like (never set)', () => {
    const c = cell({ raw_value: '=SUM(A1:A5)' });
    delete (c as any).computed_value;
    expect(cellKind(c, 'number')).toBe('raw');
  });

  it('a formula cell with computed_value starting with "#" but otherwise arbitrary text is still "err"', () => {
    expect(cellKind(cell({ raw_value: '=X', computed_value: '#REF!' }), 'text')).toBe('err');
  });

  // #2339 PR 11 — ShMismatch classifier wiring (Matt's ruling 2026-09-04,
  // "ShMismatch is IN"). `isTypeMismatch` itself is unit-tested exhaustively
  // in sheetView.test.ts — these tests only cover cellKind()'s WIRING
  // (check order, choices passthrough), not every type's parse rule.
  it('returns "mis" for a non-formula value that fails the column type\'s parse (number)', () => {
    expect(cellKind(cell({ raw_value: 'TBD' }), 'number')).toBe('mis');
  });

  it('returns "normal" for a non-formula value that parses fine for the column type', () => {
    expect(cellKind(cell({ raw_value: '42' }), 'number')).toBe('normal');
  });

  it('never returns "mis" for a formula cell, even if its raw text would fail the mismatch check', () => {
    // A formula's own literal text is never run through isTypeMismatch —
    // formula branches resolve first, unconditionally.
    expect(cellKind(cell({ raw_value: '=A1+B1', computed_value: '3' }), 'number')).toBe('calc');
  });

  it('passes the choices param through to the select mismatch check', () => {
    expect(cellKind(cell({ raw_value: 'Purple' }), 'select', ['Red', 'Green'])).toBe('mis');
    expect(cellKind(cell({ raw_value: 'Red' }), 'select', ['Red', 'Green'])).toBe('normal');
  });

  it('treats an empty value as never mismatched, regardless of column type', () => {
    expect(cellKind(cell({ raw_value: '' }), 'number')).toBe('normal');
    expect(cellKind(cell({ raw_value: '' }), 'date')).toBe('normal');
  });
});

describe('displayValue', () => {
  it('returns empty string for a null cell', () => {
    expect(displayValue(null)).toBe('');
  });

  it('returns computed_value when present', () => {
    expect(displayValue(cell({ raw_value: '=1+1', computed_value: '2' }))).toBe('2');
  });

  it('falls back to raw_value when computed_value is absent', () => {
    expect(displayValue(cell({ raw_value: '=1+1' }))).toBe('=1+1');
  });

  it('falls back to raw_value for a plain non-formula cell', () => {
    expect(displayValue(cell({ raw_value: 'plain text' }))).toBe('plain text');
  });

  it('returns empty string for a cell with an empty raw_value and no computed_value', () => {
    expect(displayValue(cell({ raw_value: '' }))).toBe('');
  });
});

describe('cellClass', () => {
  const NO_SELECTION = { anchor: null, range: null };

  it('base "sh-c" class for a normal text cell with no selection', () => {
    const cls = cellClass(0, 0, cell({ raw_value: 'hi' }), 'text', NO_SELECTION);
    expect(cls).toContain('sh-c');
    expect(cls).toContain('sh-c--text');
    expect(cls).not.toContain('is-anchor');
    expect(cls).not.toContain('is-range');
  });

  it('adds the per-type alignment class for number/currency/date/checkbox', () => {
    expect(cellClass(0, 0, cell(), 'number', NO_SELECTION)).toContain('sh-c--num');
    expect(cellClass(0, 0, cell(), 'currency', NO_SELECTION)).toContain('sh-c--currency');
    expect(cellClass(0, 0, cell(), 'date', NO_SELECTION)).toContain('sh-c--date');
    expect(cellClass(0, 0, cell(), 'checkbox', NO_SELECTION)).toContain('sh-c--check');
  });

  it('adds the type-alignment class for a resolved formula cell, but no formula-indicator class (#2874 item 55 — sh-c--calc removed, no formula indicators anywhere)', () => {
    const cls = cellClass(0, 0, cell({ raw_value: '=1+1', computed_value: '2' }), 'number', NO_SELECTION);
    expect(cls).toContain('sh-c--num');
    expect(cls).not.toContain('sh-c--calc');
  });

  it('adds sh-c--raw (no type-alignment class) for a formula cell with no computed_value', () => {
    const cls = cellClass(0, 0, cell({ raw_value: '=1+1' }), 'number', NO_SELECTION);
    expect(cls).toContain('sh-c--raw');
    expect(cls).not.toContain('sh-c--calc');
    expect(cls).not.toContain('sh-c--num');
  });

  it('adds sh-c--err (not a type-alignment class) for an error formula cell', () => {
    const cls = cellClass(0, 0, cell({ raw_value: '=1/0', computed_value: '#DIV/0!' }), 'number', NO_SELECTION);
    expect(cls).toContain('sh-c--err');
    expect(cls).not.toContain('sh-c--num');
  });

  it('adds is-anchor when the coordinate matches the selection anchor', () => {
    const cls = cellClass(1, 2, null, 'text', { anchor: { row: 1, col: 2 }, range: null });
    expect(cls).toContain('is-anchor');
  });

  it('adds is-range and edge classes when the coordinate is inside an active range', () => {
    const cls = cellClass(0, 0, null, 'text', { anchor: { row: 0, col: 0 }, range: { r0: 0, r1: 1, c0: 0, c1: 1 } });
    expect(cls).toContain('is-range');
    expect(cls).toContain('is-redge-t');
    expect(cls).toContain('is-redge-l');
  });

  it('does not add is-range when the coordinate is outside the active range', () => {
    const cls = cellClass(5, 5, null, 'text', { anchor: null, range: { r0: 0, r1: 1, c0: 0, c1: 1 } });
    expect(cls).not.toContain('is-range');
  });

  // #2339 PR 11 — ShMismatch
  it('adds sh-c--mis (not a type-alignment class) for a mismatched value', () => {
    const cls = cellClass(0, 0, cell({ raw_value: 'TBD' }), 'number', NO_SELECTION);
    expect(cls).toContain('sh-c--mis');
    expect(cls).not.toContain('sh-c--num');
  });

  it('threads the choices param through cellClass into the mismatch check', () => {
    const clsMismatch = cellClass(0, 0, cell({ raw_value: 'Purple' }), 'select', NO_SELECTION, ['Red', 'Green']);
    expect(clsMismatch).toContain('sh-c--mis');
    const clsMatch = cellClass(0, 0, cell({ raw_value: 'Red' }), 'select', NO_SELECTION, ['Red', 'Green']);
    expect(clsMatch).not.toContain('sh-c--mis');
  });
});

// #2339 PR 15b-alpha (alpha) — `cellInlineStyle()`, live-grid rendering of
// `style_json` (scope report §3.4). The CRITICAL, dedicated-test-covered
// requirement: a user's ink (`fg`) must NEVER be applied to an error-kind
// cell (inline-style specificity would otherwise silently override
// `.sh-c--err { color: var(--status-danger) }`), while `bg`/`bd` apply
// unconditionally regardless of kind.
describe('cellInlineStyle', () => {
  it('returns an empty string for null styleJson', () => {
    expect(cellInlineStyle(null, 'normal')).toBe('');
  });

  it('returns an empty string for undefined styleJson', () => {
    expect(cellInlineStyle(undefined, 'normal')).toBe('');
  });

  it('returns an empty string for an empty-string styleJson', () => {
    expect(cellInlineStyle('', 'normal')).toBe('');
  });

  it('returns an empty string for malformed (non-JSON) styleJson', () => {
    expect(cellInlineStyle('{not valid json', 'normal')).toBe('');
  });

  it('returns an empty string for a JSON object with no recognized style keys', () => {
    expect(cellInlineStyle('{"unrelated":"value"}', 'normal')).toBe('');
  });

  it('bg-only: emits a background declaration, nothing else', () => {
    const out = cellInlineStyle('{"bg":"#112233"}', 'normal');
    expect(out).toBe('background:#112233');
  });

  it('fg-only (non-error kind): emits a color declaration, nothing else', () => {
    const out = cellInlineStyle('{"fg":"#ffffff"}', 'normal');
    expect(out).toBe('color:#ffffff');
  });

  it('bd-only (single side): emits the matching --sh-c-border-shadow custom property', () => {
    const out = cellInlineStyle('{"bd":{"side":"top","w":2,"c":"#ff0000"}}', 'normal');
    expect(out).toBe('--sh-c-border-shadow:inset 0 2px 0 0 #ff0000');
  });

  it('bg+fg (non-error kind): emits both declarations, background first', () => {
    const out = cellInlineStyle('{"bg":"#112233","fg":"#ffffff"}', 'normal');
    expect(out).toBe('background:#112233;color:#ffffff');
  });

  it('bg+fg+bd (non-error kind): emits all three declarations in bg/fg/border-shadow order', () => {
    const out = cellInlineStyle('{"bg":"#112233","fg":"#ffffff","bd":{"side":"left","w":1,"c":"#00e4e4"}}', 'calc');
    expect(out).toBe('background:#112233;color:#ffffff;--sh-c-border-shadow:inset 1px 0 0 0 #00e4e4');
  });

  // ── CRITICAL error carve-out (Matt's hard requirement) ──────────────

  it('error cell with fg: does NOT emit a color declaration (error text always wins)', () => {
    const out = cellInlineStyle('{"fg":"#ffffff"}', 'err');
    expect(out).toBe('');
    expect(out).not.toContain('color');
  });

  it('error cell with bg: DOES emit the background declaration (fills apply unconditionally)', () => {
    const out = cellInlineStyle('{"bg":"#331111"}', 'err');
    expect(out).toBe('background:#331111');
  });

  it('error cell with bd: DOES emit the border-shadow custom property (borders apply unconditionally)', () => {
    const out = cellInlineStyle('{"bd":{"side":"bottom","w":2,"c":"#ef4444"}}', 'err');
    expect(out).toBe('--sh-c-border-shadow:inset 0 -2px 0 0 #ef4444;border-bottom:none');
  });

  it('error cell with bg+fg+bd: emits background and border-shadow, filters out color only', () => {
    const out = cellInlineStyle('{"bg":"#331111","fg":"#ffffff","bd":{"side":"right","w":1,"c":"#ef4444"}}', 'err');
    expect(out).toBe('background:#331111;--sh-c-border-shadow:inset -1px 0 0 0 #ef4444;border-right:none');
    expect(out).not.toContain('color:');
  });

  it('non-error kinds (raw, mis) still apply fg normally — the carve-out is err-specific, not blanket', () => {
    expect(cellInlineStyle('{"fg":"#ffffff"}', 'raw')).toBe('color:#ffffff');
    expect(cellInlineStyle('{"fg":"#ffffff"}', 'mis')).toBe('color:#ffffff');
  });
});

// #2874 item 8 — solid borders render as an inset box-shadow via the
// --sh-c-border-shadow custom property (composed with SheetGrid.svelte's
// state-class box-shadow rules), not literal border-{side} CSS, so
// per-side weights (1/2/3px) don't collapse against the adjacent cell's
// own border or the base `.sh-c` hairline.
describe('cellInlineStyle — box-shadow border rendering (#2874 item 8)', () => {
  it('two-side border (top+right, weight 2): emits comma-joined inset box-shadow in deterministic order', () => {
    const json = JSON.stringify({
      bd: [
        { side: 'top', w: 2, c: 'rgba(255,255,255,0.18)' },
        { side: 'right', w: 2, c: 'rgba(255,255,255,0.18)' },
      ],
    });
    const out = cellInlineStyle(json, 'normal');
    expect(out).toBe(
      '--sh-c-border-shadow:inset 0 2px 0 0 rgba(255,255,255,0.18), inset -2px 0 0 0 rgba(255,255,255,0.18);border-right:none'
    );
  });

  it('four-side border (all sides, weight 1): emits all four inset segments in top/bottom/left/right order', () => {
    const json = JSON.stringify({
      bd: [
        { side: 'top', w: 1, c: 'rgba(255,255,255,0.18)' },
        { side: 'right', w: 1, c: 'rgba(255,255,255,0.18)' },
        { side: 'bottom', w: 1, c: 'rgba(255,255,255,0.18)' },
        { side: 'left', w: 1, c: 'rgba(255,255,255,0.18)' },
      ],
    });
    const out = cellInlineStyle(json, 'normal');
    expect(out).toBe(
      '--sh-c-border-shadow:inset 0 1px 0 0 rgba(255,255,255,0.18), inset 0 -1px 0 0 rgba(255,255,255,0.18), ' +
      'inset 1px 0 0 0 rgba(255,255,255,0.18), inset -1px 0 0 0 rgba(255,255,255,0.18);border-right:none;border-bottom:none'
    );
  });

  it('styled bottom+right border suppresses base hairlines on those edges', () => {
    const json = JSON.stringify({ bd: [{ side: 'bottom', w: 1, c: 'rgba(255,255,255,0.18)' }, { side: 'right', w: 1, c: 'rgba(255,255,255,0.18)' }] });
    const out = cellInlineStyle(json, 'normal');
    expect(out).toContain('border-bottom:none');
    expect(out).toContain('border-right:none');
  });

  it('styled top-only border does NOT suppress right/bottom hairlines', () => {
    const json = JSON.stringify({ bd: [{ side: 'top', w: 1, c: 'rgba(255,255,255,0.18)' }] });
    const out = cellInlineStyle(json, 'normal');
    expect(out).not.toContain('border-bottom');
    expect(out).not.toContain('border-right');
  });
});

// #2874 item 38 (Matt's ruling 2026-09-15): currency cells render via
// Intl.NumberFormat('en-US', ...) — symbol + two fixed decimals
// (`$930.00`), not the raw decimal value + a separate ISO-code span.
// `currencyCode()`/`formatCurrencyValue()` are the SHARED formatter both
// SheetGrid.svelte (live grid) and ShSnapshotPreview.svelte (version-
// history preview) import — no duplicated currency lookup/formatting
// logic (ruling 3).
describe('currencyCode', () => {
  it('reads the "currency" key from options_json', () => {
    expect(currencyCode('{"currency":"EUR"}')).toBe('EUR');
  });

  it('falls back to the "code" key when "currency" is absent', () => {
    expect(currencyCode('{"code":"GBP"}')).toBe('GBP');
  });

  it('defaults to USD for empty options_json', () => {
    expect(currencyCode('')).toBe('USD');
    expect(currencyCode('{}')).toBe('USD');
  });

  it('defaults to USD for malformed (non-JSON) options_json', () => {
    expect(currencyCode('{not valid json')).toBe('USD');
  });
});

describe('formatCurrencyValue', () => {
  it('formats a plain USD decimal with the $ symbol and two decimals', () => {
    expect(formatCurrencyValue('930', '{"currency":"USD"}')).toBe('$930.00');
  });

  it('pads a single-decimal value to two decimals', () => {
    expect(formatCurrencyValue('930.5', '{"currency":"USD"}')).toBe('$930.50');
  });

  it('rounds a value with more than two decimals (banker-neutral half-up via Intl)', () => {
    expect(formatCurrencyValue('930.567', '{"currency":"USD"}')).toBe('$930.57');
  });

  it('formats a non-USD currency with its own symbol', () => {
    expect(formatCurrencyValue('930', '{"currency":"EUR"}')).toBe('\u20ac930.00');
    expect(formatCurrencyValue('930', '{"currency":"GBP"}')).toBe('\u00a3930.00');
  });

  it('defaults to USD/$ when options_json is empty or malformed', () => {
    expect(formatCurrencyValue('930', '')).toBe('$930.00');
    expect(formatCurrencyValue('930', '{not valid json')).toBe('$930.00');
  });

  it('handles a negative value correctly', () => {
    expect(formatCurrencyValue('-45.5', '{"currency":"USD"}')).toBe('-$45.50');
  });

  it('handles a zero value correctly', () => {
    expect(formatCurrencyValue('0', '{"currency":"USD"}')).toBe('$0.00');
  });

  it('falls back to the raw value unchanged for a non-numeric/unparseable value', () => {
    expect(formatCurrencyValue('#DIV/0!', '{"currency":"USD"}')).toBe('#DIV/0!');
  });

  it('returns an empty string for an empty raw value', () => {
    expect(formatCurrencyValue('', '{"currency":"USD"}')).toBe('');
  });
});
