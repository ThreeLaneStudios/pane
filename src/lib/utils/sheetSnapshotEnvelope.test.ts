// sheetSnapshotEnvelope.test.ts — unit tests for the version-history
// snapshot envelope parser/pivot/style-parser (#2339 PR 15b-beta).

import { describe, it, expect } from 'vitest';
import {
  parseSheetSnapshotEnvelope,
  pivotSnapshotEnvelope,
  parseCellStyleJson,
  type SheetSnapshotEnvelope,
} from './sheetSnapshotEnvelope';

function envelope(overrides: Partial<SheetSnapshotEnvelope> = {}): SheetSnapshotEnvelope {
  return {
    v: 1,
    columns: [
      { id: 'col-a', name: 'Line item', col_type: 'text', options_json: '{}', width: 164, sort_order: 0 },
      { id: 'col-b', name: 'Total', col_type: 'currency', options_json: '{}', width: 118, sort_order: 1 },
    ],
    rows: [
      { id: 'row-1', sort_order: 0, height: 32 },
      { id: 'row-2', sort_order: 1, height: 0 },
    ],
    cells: [
      { row_id: 'row-1', column_id: 'col-a', value: 'Longform essays', style_json: '{}' },
      { row_id: 'row-1', column_id: 'col-b', value: '=B1*2', style_json: '{}' },
    ],
    ...overrides,
  };
}

describe('parseSheetSnapshotEnvelope', () => {
  it('parses a valid v1 envelope', () => {
    const json = JSON.stringify(envelope());
    const parsed = parseSheetSnapshotEnvelope(json);
    expect(parsed).not.toBeNull();
    expect(parsed?.v).toBe(1);
    expect(parsed?.columns).toHaveLength(2);
    expect(parsed?.rows).toHaveLength(2);
    expect(parsed?.cells).toHaveLength(2);
  });

  it('returns null on malformed JSON', () => {
    expect(parseSheetSnapshotEnvelope('{not json')).toBeNull();
  });

  it('returns null on a non-object JSON value', () => {
    expect(parseSheetSnapshotEnvelope('42')).toBeNull();
    expect(parseSheetSnapshotEnvelope('null')).toBeNull();
    expect(parseSheetSnapshotEnvelope('"a string"')).toBeNull();
  });

  it('returns null when v is missing', () => {
    expect(parseSheetSnapshotEnvelope(JSON.stringify({ columns: [], rows: [], cells: [] }))).toBeNull();
  });

  it('returns null on an unrecognized v value', () => {
    const json = JSON.stringify({ ...envelope(), v: 2 });
    expect(parseSheetSnapshotEnvelope(json)).toBeNull();
  });

  it('returns null when any required array is missing', () => {
    expect(parseSheetSnapshotEnvelope(JSON.stringify({ v: 1, rows: [], cells: [] }))).toBeNull();
    expect(parseSheetSnapshotEnvelope(JSON.stringify({ v: 1, columns: [], cells: [] }))).toBeNull();
    expect(parseSheetSnapshotEnvelope(JSON.stringify({ v: 1, columns: [], rows: [] }))).toBeNull();
  });
});

describe('pivotSnapshotEnvelope', () => {
  it('produces one dense row per envelope row, one dense cell per column', () => {
    const pivoted = pivotSnapshotEnvelope(envelope());
    expect(pivoted.rows).toHaveLength(2);
    expect(pivoted.rows[0].cells).toHaveLength(2);
    expect(pivoted.rows[1].cells).toHaveLength(2);
  });

  it('places sparse cells at the correct dense coordinate', () => {
    const pivoted = pivotSnapshotEnvelope(envelope());
    expect(pivoted.rows[0].cells[0].cell?.value).toBe('Longform essays');
    expect(pivoted.rows[0].cells[1].cell?.value).toBe('=B1*2');
    expect(pivoted.rows[1].cells[0].cell).toBeNull();
    expect(pivoted.rows[1].cells[1].cell).toBeNull();
  });

  it('applies the row-height-0 fallback (32px)', () => {
    const pivoted = pivotSnapshotEnvelope(envelope());
    expect(pivoted.rows[0].height).toBe(32);
    expect(pivoted.rows[1].height).toBe(32); // was 0, fell back
  });

  it('applies the column-width-0 fallback (148px)', () => {
    const withZeroWidth = envelope({
      columns: [{ id: 'col-a', name: 'X', col_type: 'text', options_json: '{}', width: 0, sort_order: 0 }],
    });
    const pivoted = pivotSnapshotEnvelope(withZeroWidth);
    expect(pivoted.columns[0].effectiveWidth).toBe(148);
  });

  it('counts formula cells (=-prefixed value) for the "K computed" metric', () => {
    const pivoted = pivotSnapshotEnvelope(envelope());
    expect(pivoted.computedCount).toBe(1);
  });

  it('sorts columns and rows by sort_order regardless of input array order', () => {
    const scrambled = envelope({
      columns: [
        { id: 'col-b', name: 'Total', col_type: 'currency', options_json: '{}', width: 118, sort_order: 1 },
        { id: 'col-a', name: 'Line item', col_type: 'text', options_json: '{}', width: 164, sort_order: 0 },
      ],
    });
    const pivoted = pivotSnapshotEnvelope(scrambled);
    expect(pivoted.columns[0].id).toBe('col-a');
    expect(pivoted.columns[1].id).toBe('col-b');
  });

  it('never stores a computed value — a formula cell\'s dense entry carries only the raw formula string', () => {
    const pivoted = pivotSnapshotEnvelope(envelope());
    const formulaCell = pivoted.rows[0].cells[1].cell;
    expect(formulaCell?.value).toBe('=B1*2');
    expect(formulaCell).not.toHaveProperty('computed_value');
  });
});

describe('parseCellStyleJson', () => {
  it('parses a bg-only style', () => {
    expect(parseCellStyleJson('{"bg":"rgba(239,68,68,0.13)"}')).toEqual({ background: 'rgba(239,68,68,0.13)' });
  });

  it('parses an fg-only style', () => {
    expect(parseCellStyleJson('{"fg":"#EF4444"}')).toEqual({ color: '#EF4444' });
  });

  it('parses a top-border style', () => {
    expect(parseCellStyleJson('{"bd":{"side":"top","w":2,"c":"var(--border-strong)"}}')).toEqual({
      // #2874 item 8: parseCellStyleJson migrates any legacy stored
      // var(--border-strong) border color to the new --sheet-border-styled
      // token on read, so old sheets pick it up without a data migration.
      boxShadow: 'inset 0 2px 0 0 var(--sheet-border-styled)',
    });
  });

  it('parses combined bg + fg + border', () => {
    const parsed = parseCellStyleJson('{"bg":"#232327","fg":"#F0EDE8","bd":{"side":"top","w":2,"c":"var(--accent)"}}');
    expect(parsed).toEqual({ background: '#232327', color: '#F0EDE8', boxShadow: 'inset 0 2px 0 0 var(--accent)' });
  });

  it('returns {} for empty/null/undefined input', () => {
    expect(parseCellStyleJson('')).toEqual({});
    expect(parseCellStyleJson(null)).toEqual({});
    expect(parseCellStyleJson(undefined)).toEqual({});
  });

  it('returns {} for malformed JSON without throwing', () => {
    expect(() => parseCellStyleJson('{not json')).not.toThrow();
    expect(parseCellStyleJson('{not json')).toEqual({});
  });

  it('returns {} for an ad-hoc unrecognized shape (defensive, no canonical schema)', () => {
    expect(parseCellStyleJson('{"someFutureKey":"value"}')).toEqual({});
  });

  it('ignores an unrecognized border side', () => {
    expect(parseCellStyleJson('{"bd":{"side":"diagonal","w":2,"c":"red"}}')).toEqual({});
  });

  // #2339 PR 15d-beta: `bd` as a multi-border ARRAY (buildCellStyleJson's
  // write format, one entry per styled side) plus backward compat with
  // alpha's original single-object `bd` shape (already covered above).
  //
  // #2874 item 8: solid sides now render as a single deterministically-
  // ordered (top/bottom/left/right) `boxShadow` string rather than four
  // literal `border-*` fields; a `dashed:true` entry still falls back to
  // the original literal `border-{side}` field (box-shadow has no dashed
  // equivalent).
  it('parses a multi-border array with all four sides', () => {
    const json = '{"bd":[' +
      '{"side":"top","w":1,"c":"var(--border-strong)"},' +
      '{"side":"right","w":2,"c":"#EF4444"},' +
      '{"side":"bottom","w":3,"c":"var(--accent)","dashed":true},' +
      '{"side":"left","w":1,"c":"#000"}' +
      ']}';
    expect(parseCellStyleJson(json)).toEqual({
      // #2874 item 8: legacy var(--border-strong) input is migrated to
      // var(--sheet-border-styled) on read (see 'parses a top-border style').
      boxShadow: 'inset 0 1px 0 0 var(--sheet-border-styled), inset 1px 0 0 0 #000, inset -2px 0 0 0 #EF4444',
      styledRight: true,
      borderBottom: '3px dashed var(--accent)',
    });
  });

  it('parses a single-entry bd array (the common one-side-styled case)', () => {
    expect(parseCellStyleJson('{"bd":[{"side":"top","w":2,"c":"var(--border-strong)"}]}')).toEqual({
      // #2874 item 8: legacy var(--border-strong) input is migrated to
      // var(--sheet-border-styled) on read (see 'parses a top-border style').
      boxShadow: 'inset 0 2px 0 0 var(--sheet-border-styled)',
    });
  });

  it('an empty bd array yields no border styles', () => {
    expect(parseCellStyleJson('{"bd":[]}')).toEqual({});
  });

  it('skips malformed entries within a bd array without dropping the valid ones', () => {
    const json = '{"bd":[{"side":"top","w":2,"c":"red"},{"side":"bogus"},null,{"side":"left","w":1,"c":"blue"}]}';
    expect(parseCellStyleJson(json)).toEqual({
      boxShadow: 'inset 0 2px 0 0 red, inset 1px 0 0 0 blue',
    });
  });

  it('renders a dashed border style when dashed:true (array format)', () => {
    expect(parseCellStyleJson('{"bd":[{"side":"top","w":2,"c":"red","dashed":true}]}')).toEqual({
      borderTop: '2px dashed red',
    });
  });

  it('backward compat: the legacy single-object bd shape (alpha-era style_json) still parses identically after the beta parser update', () => {
    // Same assertion as the pre-existing 'parses a top-border style' test
    // above, restated here explicitly as a backward-compat regression
    // guard alongside the new array-format tests.
    expect(parseCellStyleJson('{"bd":{"side":"bottom","w":1,"c":"#fff"}}')).toEqual({
      boxShadow: 'inset 0 -1px 0 0 #fff',
      styledBottom: true,
    });
  });

  it('migrates legacy var(--border-strong) border color to var(--sheet-border-styled) on read (#2874 item 8)', () => {
    const parsed = parseCellStyleJson('{"bd":{"side":"top","w":2,"c":"var(--border-strong)"}}');
    expect(parsed.boxShadow).toContain('var(--sheet-border-styled)');
    expect(parsed.boxShadow).not.toContain('var(--border-strong)');
  });
});
