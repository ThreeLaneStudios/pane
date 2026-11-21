// sheetCellStyle.test.ts — unit tests for the Sheets cell-styling WRITE
// path (#2339 PR 15d-beta). Every test drives the real exported
// functions against real-shaped fixtures — no mocking.

import { describe, it, expect } from 'vitest';
import {
  buildCellStyleJson,
  mergeStylePatch,
  expandSelection,
  detectUnanimity,
  detectBorderUnanimity,
  parseCellStyleRaw,
  resolvedBorderSides,
  type CellStyleInput,
  type BorderSpec,
} from './sheetCellStyle';
import { parseCellStyleJson } from './sheetSnapshotEnvelope';

describe('buildCellStyleJson', () => {
  it('returns "{}" for a fully-empty style', () => {
    expect(buildCellStyleJson({})).toBe('{}');
  });

  it('returns "{}" when every field is explicitly null/empty', () => {
    expect(buildCellStyleJson({ bg: null, fg: null, bd: null })).toBe('{}');
    expect(buildCellStyleJson({ bg: '', fg: '', bd: [] })).toBe('{}');
  });

  it('emits bg only', () => {
    expect(JSON.parse(buildCellStyleJson({ bg: '#232327' }))).toEqual({ bg: '#232327' });
  });

  it('emits fg only', () => {
    expect(JSON.parse(buildCellStyleJson({ fg: '#EF4444' }))).toEqual({ fg: '#EF4444' });
  });

  it('emits bd as an array, even for a single border', () => {
    const bd: BorderSpec[] = [{ side: 'top', w: 2, c: '#ff0000' }];
    const out = JSON.parse(buildCellStyleJson({ bd }));
    expect(Array.isArray(out.bd)).toBe(true);
    expect(out.bd).toEqual(bd);
  });

  it('emits bg+fg+bd together', () => {
    const bd: BorderSpec[] = [{ side: 'left', w: 1, c: '#00e4e4', dashed: true }];
    const out = JSON.parse(buildCellStyleJson({ bg: '#232327', fg: '#F0EDE8', bd }));
    expect(out).toEqual({ bg: '#232327', fg: '#F0EDE8', bd });
  });

  it('emits multiple border sides in one array (multi-border support)', () => {
    const bd: BorderSpec[] = [
      { side: 'top', w: 2, c: '#ff0000' },
      { side: 'left', w: 1, c: '#00e4e4' },
    ];
    const out = JSON.parse(buildCellStyleJson({ bd }));
    expect(out.bd).toHaveLength(2);
    expect(out.bd).toEqual(bd);
  });

  it('round-trips through parseCellStyleJson (the READ path) — single border', () => {
    const json = buildCellStyleJson({ bg: '#112233', fg: '#ffffff', bd: [{ side: 'top', w: 2, c: '#ff0000' }] });
    const parsed = parseCellStyleJson(json);
    expect(parsed).toEqual({ background: '#112233', color: '#ffffff', boxShadow: 'inset 0 2px 0 0 #ff0000' });
  });

  it('round-trips through parseCellStyleJson — multiple borders on one cell', () => {
    const json = buildCellStyleJson({
      bd: [
        { side: 'top', w: 2, c: '#ff0000' },
        { side: 'bottom', w: 1, c: '#00e4e4', dashed: true },
      ],
    });
    const parsed = parseCellStyleJson(json);
    expect(parsed.boxShadow).toBe('inset 0 2px 0 0 #ff0000');
    expect(parsed.borderBottom).toBe('1px dashed #00e4e4');
  });

  it('round-trips an empty style back to an empty parse', () => {
    expect(parseCellStyleJson(buildCellStyleJson({}))).toEqual({});
  });
});

describe('parseCellStyleRaw', () => {
  it('returns {} for null/undefined/empty/malformed input', () => {
    expect(parseCellStyleRaw(null)).toEqual({});
    expect(parseCellStyleRaw(undefined)).toEqual({});
    expect(parseCellStyleRaw('')).toEqual({});
    expect(parseCellStyleRaw('{not json')).toEqual({});
  });

  it('parses bg/fg', () => {
    expect(parseCellStyleRaw('{"bg":"#112233","fg":"#ffffff"}')).toEqual({ bg: '#112233', fg: '#ffffff' });
  });

  it('parses the beta multi-border array format', () => {
    const out = parseCellStyleRaw('{"bd":[{"side":"top","w":2,"c":"#ff0000"},{"side":"left","w":1,"c":"#00e4e4"}]}');
    expect(out.bd).toEqual([
      { side: 'top', w: 2, c: '#ff0000' },
      { side: 'left', w: 1, c: '#00e4e4' },
    ]);
  });

  it('parses the legacy alpha single-object bd format, wrapping it into a one-entry array', () => {
    const out = parseCellStyleRaw('{"bd":{"side":"bottom","w":2,"c":"#ef4444"}}');
    expect(out.bd).toEqual([{ side: 'bottom', w: 2, c: '#ef4444' }]);
  });

  it('drops malformed entries inside a bd array rather than throwing', () => {
    const out = parseCellStyleRaw('{"bd":[{"side":"top","w":2,"c":"#ff0000"},{"bad":"entry"}]}');
    expect(out.bd).toEqual([{ side: 'top', w: 2, c: '#ff0000' }]);
  });
});

describe('mergeStylePatch', () => {
  it('leaves untouched keys (absent from the patch) alone', () => {
    const existing: CellStyleInput = { bg: '#112233', fg: '#ffffff' };
    const merged = mergeStylePatch(existing, {});
    expect(merged).toEqual(existing);
  });

  it('replaces a key with the patch value when present', () => {
    const existing: CellStyleInput = { bg: '#112233' };
    const merged = mergeStylePatch(existing, { bg: '#445566' });
    expect(merged.bg).toBe('#445566');
  });

  it('clears a key when the patch explicitly sets it to null', () => {
    const existing: CellStyleInput = { bg: '#112233', fg: '#ffffff' };
    const merged = mergeStylePatch(existing, { fg: null });
    expect(merged).toEqual({ bg: '#112233', fg: null });
  });

  it('adds a new key not present on the existing style', () => {
    const existing: CellStyleInput = { bg: '#112233' };
    const merged = mergeStylePatch(existing, { fg: '#EF4444' });
    expect(merged).toEqual({ bg: '#112233', fg: '#EF4444' });
  });

  it('replaces the bd array wholesale rather than merging entries', () => {
    const existing: CellStyleInput = { bd: [{ side: 'top', w: 2, c: '#ff0000' }] };
    const merged = mergeStylePatch(existing, { bd: [{ side: 'left', w: 1, c: '#00e4e4' }] });
    expect(merged.bd).toEqual([{ side: 'left', w: 1, c: '#00e4e4' }]);
  });

  it('clears bd when the patch sets it to null', () => {
    const existing: CellStyleInput = { bd: [{ side: 'top', w: 2, c: '#ff0000' }] };
    const merged = mergeStylePatch(existing, { bd: null });
    expect(merged.bd).toBeNull();
  });
});

describe('expandSelection', () => {
  it('returns [] when there is no anchor', () => {
    expect(expandSelection({ anchor: null, range: null })).toEqual([]);
  });

  it('returns just the anchor when there is no range', () => {
    expect(expandSelection({ anchor: { row: 2, col: 3 }, range: null })).toEqual([{ row: 2, col: 3 }]);
  });

  it('expands a range into every coordinate in the inclusive rectangle', () => {
    const coords = expandSelection({ anchor: { row: 0, col: 0 }, range: { r0: 0, r1: 1, c0: 0, c1: 1 } });
    expect(coords).toEqual([
      { row: 0, col: 0 },
      { row: 0, col: 1 },
      { row: 1, col: 0 },
      { row: 1, col: 1 },
    ]);
  });

  it('expands a non-square range correctly', () => {
    const coords = expandSelection({ anchor: { row: 0, col: 0 }, range: { r0: 0, r1: 2, c0: 0, c1: 0 } });
    expect(coords).toHaveLength(3);
    expect(coords).toEqual([{ row: 0, col: 0 }, { row: 1, col: 0 }, { row: 2, col: 0 }]);
  });

  it('a single-cell range (r0===r1, c0===c1) still returns exactly one coordinate', () => {
    const coords = expandSelection({ anchor: { row: 1, col: 1 }, range: { r0: 1, r1: 1, c0: 1, c1: 1 } });
    expect(coords).toEqual([{ row: 1, col: 1 }]);
  });
});

describe('detectUnanimity', () => {
  it('returns null for an empty cell list', () => {
    expect(detectUnanimity([], 'bg')).toBeNull();
  });

  it('returns the shared value when every cell agrees', () => {
    const cells = [{ style_json: '{"bg":"#112233"}' }, { style_json: '{"bg":"#112233"}' }];
    expect(detectUnanimity(cells, 'bg')).toBe('#112233');
  });

  it('returns null when cells disagree', () => {
    const cells = [{ style_json: '{"bg":"#112233"}' }, { style_json: '{"bg":"#445566"}' }];
    expect(detectUnanimity(cells, 'bg')).toBeNull();
  });

  it('treats an absent key (no styling) as null, agreeing with other unset cells', () => {
    const cells = [{ style_json: '{}' }, { style_json: '' }];
    expect(detectUnanimity(cells, 'fg')).toBeNull();
    // both are "unset" so they agree — the unanimous value is null itself,
    // meaning "no fill" is the unanimous state (still counts as unanimity,
    // it's just that the panel renders no swatch highlighted for null).
  });

  it('a single cell is trivially unanimous with itself', () => {
    expect(detectUnanimity([{ style_json: '{"fg":"#EF4444"}' }], 'fg')).toBe('#EF4444');
  });

  it('one styled + one unstyled cell is NOT unanimous', () => {
    const cells = [{ style_json: '{"bg":"#112233"}' }, { style_json: '{}' }];
    expect(detectUnanimity(cells, 'bg')).toBeNull();
  });

  it('checks the fg key independently of bg', () => {
    const cells = [{ style_json: '{"bg":"#112233","fg":"#ffffff"}' }, { style_json: '{"bg":"#445566","fg":"#ffffff"}' }];
    expect(detectUnanimity(cells, 'bg')).toBeNull();
    expect(detectUnanimity(cells, 'fg')).toBe('#ffffff');
  });
});

// #2874 item 45 (2026-09-24): `detectBorderUnanimity` used to return
// `BorderUnanimity | null`, and `null` was used for two opposite things
// indistinguishably — "no cell has this side" and "cells disagree on this
// side". It now returns a real three-state `BorderSideResult` —
// `{state:'none'}` / `{state:'unanimous', w, c, dashed}` /
// `{state:'mixed'}` — so callers can tell those apart. REWRITTEN (not
// merely supplemented) as part of item 45's fix, since the five tests
// below used to assert `toBeNull()` for scenarios that are actually
// "mixed", pinning the bug as correct: 'returns null when cells have
// different widths on the same side', 'returns null when cells have
// different colors on the same side', 'returns null when one cell has the
// border and another does not', 'respects dashed as part of the
// unanimity comparison', and the `left`-side assertion inside 'checks
// only the requested side, ignoring other borders on the same cells' —
// see the PR description for the full rewritten-test list. The other
// three tests (empty list, no side styled, every cell agrees) keep their
// original semantics but were also touched here since the return SHAPE
// changed for every case, not just the mixed ones.
describe('detectBorderUnanimity', () => {
  it('returns none for an empty cell list', () => {
    expect(detectBorderUnanimity([], 'top')).toEqual({ state: 'none' });
  });

  it('returns none when no cell has the given side styled', () => {
    const cells = [{ style_json: '{}' }, { style_json: '{"bg":"#112233"}' }];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'none' });
  });

  it('returns the shared border spec, unanimous, when every cell agrees on that side', () => {
    const cells = [
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' },
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' },
    ];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'unanimous', w: 2, c: '#ff0000', dashed: false });
  });

  it('REWRITTEN (#2874 item 45): returns mixed — not null/absent — when cells have different widths on the same side', () => {
    const cells = [
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' },
      { style_json: '{"bd":[{"side":"top","w":1,"c":"#ff0000"}]}' },
    ];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'mixed' });
  });

  it('REWRITTEN (#2874 item 45): returns mixed — not null/absent — when cells have different colors on the same side', () => {
    const cells = [
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' },
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#00e4e4"}]}' },
    ];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'mixed' });
  });

  it('REWRITTEN (#2874 item 45): returns mixed — not null/absent — when one cell has the border and another does not (Matt\'s ruling: partial presence counts as mixed)', () => {
    const cells = [{ style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' }, { style_json: '{}' }];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'mixed' });
  });

  it('REWRITTEN (#2874 item 45): dashed disagreement is part of the unanimity comparison and now reports mixed, not null/absent', () => {
    const cells = [
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000","dashed":true}]}' },
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"}]}' },
    ];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'mixed' });
  });

  it('REWRITTEN (#2874 item 45): checks only the requested side, ignoring other borders on the same cells — the disagreeing side now reports mixed, not null/absent', () => {
    const cells = [
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"},{"side":"left","w":1,"c":"#00e4e4"}]}' },
      { style_json: '{"bd":[{"side":"top","w":2,"c":"#ff0000"},{"side":"left","w":3,"c":"#22c55e"}]}' },
    ];
    expect(detectBorderUnanimity(cells, 'top')).toEqual({ state: 'unanimous', w: 2, c: '#ff0000', dashed: false });
    expect(detectBorderUnanimity(cells, 'left')).toEqual({ state: 'mixed' });
  });

  it('unanimous none across every side (multi-cell, genuinely borderless selection)', () => {
    const cells = [{ style_json: '{}' }, { style_json: '{"bg":"#112233"}' }, { style_json: '{"bd":[]}' }];
    for (const side of ['top', 'bottom', 'left', 'right'] as const) {
      expect(detectBorderUnanimity(cells, side)).toEqual({ state: 'none' });
    }
  });
});

// #2339 PR 15d-beta — Matt's outer-edge border ruling (scope report
// "Matt's Rulings" §3): these tests exercise the REAL, EXPORTED
// `resolvedBorderSides` per-cell side-resolution helper — the exact same
// function `SheetGrid.svelte`'s `applyStyleToSelection` calls per
// selected cell — so this coverage is of the shipped code path, not a
// re-implementation of it.
describe("Matt's outer-edge border ruling — per-cell side resolution", () => {
  const range3x3 = { r0: 0, r1: 2, c0: 0, c1: 2 };

  it('"top" applies border-top only to the top row of a 3x3 range', () => {
    expect(resolvedBorderSides('top', { row: 0, col: 0 }, range3x3)).toEqual(['top']);
    expect(resolvedBorderSides('top', { row: 0, col: 1 }, range3x3)).toEqual(['top']);
    expect(resolvedBorderSides('top', { row: 0, col: 2 }, range3x3)).toEqual(['top']);
  });

  it('"top" does NOT apply to middle or bottom rows of a 3x3 range', () => {
    expect(resolvedBorderSides('top', { row: 1, col: 0 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('top', { row: 1, col: 1 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('top', { row: 2, col: 2 }, range3x3)).toEqual([]);
  });

  it('"bottom" applies only to the bottom row', () => {
    expect(resolvedBorderSides('bottom', { row: 2, col: 0 }, range3x3)).toEqual(['bottom']);
    expect(resolvedBorderSides('bottom', { row: 0, col: 0 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('bottom', { row: 1, col: 1 }, range3x3)).toEqual([]);
  });

  it('"left" applies only to the leftmost column', () => {
    expect(resolvedBorderSides('left', { row: 1, col: 0 }, range3x3)).toEqual(['left']);
    expect(resolvedBorderSides('left', { row: 1, col: 1 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('left', { row: 1, col: 2 }, range3x3)).toEqual([]);
  });

  it('"right" applies only to the rightmost column', () => {
    expect(resolvedBorderSides('right', { row: 1, col: 2 }, range3x3)).toEqual(['right']);
    expect(resolvedBorderSides('right', { row: 1, col: 0 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('right', { row: 1, col: 1 }, range3x3)).toEqual([]);
  });

  it('"all" applies every side to every cell, including interior cells', () => {
    expect(resolvedBorderSides('all', { row: 1, col: 1 }, range3x3)).toEqual(['top', 'bottom', 'left', 'right']);
    expect(resolvedBorderSides('all', { row: 0, col: 0 }, range3x3)).toEqual(['top', 'bottom', 'left', 'right']);
  });

  it('"none" clears every side on every cell', () => {
    expect(resolvedBorderSides('none', { row: 0, col: 0 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('none', { row: 1, col: 1 }, range3x3)).toEqual([]);
    expect(resolvedBorderSides('none', { row: 2, col: 2 }, range3x3)).toEqual([]);
  });

  it('the corner cell of a "top" application gets exactly one side (not also left/right)', () => {
    expect(resolvedBorderSides('top', { row: 0, col: 0 }, range3x3)).toEqual(['top']);
  });
});
