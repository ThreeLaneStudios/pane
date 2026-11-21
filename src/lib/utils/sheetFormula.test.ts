// sheetFormula.test.ts — #2339 PR 14 (Formula Bar & Formula Editing)
import { describe, it, expect } from 'vitest';
import {
  tokenizeFormula,
  a1ColLettersToIndex,
  extractFormulaRefRanges,
  isCellInAnyRefRange,
  refRangeEdges,
} from './sheetFormula';

describe('tokenizeFormula', () => {
  it('classifies a plain cell reference as sh-t-ref', () => {
    const tokens = tokenizeFormula('=A1');
    expect(tokens.some((t) => t.text === 'A1' && t.cls === 'sh-t-ref')).toBe(true);
  });

  it('classifies a range reference as sh-t-ref', () => {
    const tokens = tokenizeFormula('=SUM(F1:F5)');
    expect(tokens.some((t) => t.text === 'F1:F5' && t.cls === 'sh-t-ref')).toBe(true);
  });

  it('classifies a function name (immediately followed by parens) as sh-t-fn', () => {
    const tokens = tokenizeFormula('=SUM(A1:A5)');
    expect(tokens.some((t) => t.text === 'SUM' && t.cls === 'sh-t-fn')).toBe(true);
  });

  it('classifies a quoted string literal as sh-t-str (Matt ruling 2026-09-05 #1: green, not receding)', () => {
    const tokens = tokenizeFormula('=IF(A1>0, "over", "under")');
    const strTokens = tokens.filter((t) => t.cls === 'sh-t-str');
    expect(strTokens.map((t) => t.text)).toEqual(['"over"', '"under"']);
  });

  it('classifies a bare number as sh-t-num', () => {
    const tokens = tokenizeFormula('=A1*2');
    expect(tokens.some((t) => t.text === '2' && t.cls === 'sh-t-num')).toBe(true);
  });

  it('classifies operators/punctuation as sh-t-op', () => {
    const tokens = tokenizeFormula('=A1+B1');
    expect(tokens.some((t) => t.text === '+' && t.cls === 'sh-t-op')).toBe(true);
  });

  it('handles a realistic multi-token formula end to end', () => {
    const tokens = tokenizeFormula('=IF(SUM(F1:F8)>20000, "over", AVERAGE(F1:F8))');
    const classes = tokens.map((t) => `${t.cls}:${t.text}`);
    expect(classes).toContain('sh-t-fn:IF');
    expect(classes).toContain('sh-t-fn:SUM');
    expect(classes).toContain('sh-t-ref:F1:F8');
    expect(classes).toContain('sh-t-num:20000');
    expect(classes).toContain('sh-t-str:"over"');
    expect(classes).toContain('sh-t-fn:AVERAGE');
  });

  it('returns an empty array for an empty string', () => {
    expect(tokenizeFormula('')).toEqual([]);
  });
});

describe('a1ColLettersToIndex', () => {
  it('converts single letters', () => {
    expect(a1ColLettersToIndex('A')).toBe(0);
    expect(a1ColLettersToIndex('B')).toBe(1);
    expect(a1ColLettersToIndex('Z')).toBe(25);
  });

  it('converts multi-letter columns', () => {
    expect(a1ColLettersToIndex('AA')).toBe(26);
    expect(a1ColLettersToIndex('AB')).toBe(27);
  });

  it('is case-insensitive', () => {
    expect(a1ColLettersToIndex('a')).toBe(0);
    expect(a1ColLettersToIndex('ab')).toBe(27);
  });
});

describe('extractFormulaRefRanges', () => {
  it('extracts a single cell reference as a 1x1 range', () => {
    const ranges = extractFormulaRefRanges('=A1*2');
    expect(ranges).toEqual([{ c0: 0, r0: 0, c1: 0, r1: 0 }]);
  });

  it('extracts a range reference spanning multiple rows', () => {
    const ranges = extractFormulaRefRanges('=SUM(F1:F5)');
    expect(ranges).toEqual([{ c0: 5, r0: 0, c1: 5, r1: 4 }]);
  });

  it('extracts multiple distinct references from one formula', () => {
    const ranges = extractFormulaRefRanges('=A1+B1');
    expect(ranges).toEqual([
      { c0: 0, r0: 0, c1: 0, r1: 0 },
      { c0: 1, r0: 0, c1: 1, r1: 0 },
    ]);
  });

  it('does not mistake a bare function name for a reference', () => {
    const ranges = extractFormulaRefRanges('=SUM(A1:A5)');
    // Only A1:A5 should match — "SUM" has no trailing digits.
    expect(ranges).toEqual([{ c0: 0, r0: 0, c1: 0, r1: 4 }]);
  });

  it('normalizes a reversed range (end before start) into min/max order', () => {
    const ranges = extractFormulaRefRanges('=SUM(F5:F1)');
    expect(ranges).toEqual([{ c0: 5, r0: 0, c1: 5, r1: 4 }]);
  });

  it('extracts a single-row range spanning multiple columns (item 37, #2874)', () => {
    // A1-form parsing was already rectangle-generic before item 37 —
    // this locks in the single-row case (SUM(A1:C1)) alongside the
    // existing single-column case (SUM(F1:F5)) above.
    const ranges = extractFormulaRefRanges('=SUM(A1:C1)');
    expect(ranges).toEqual([{ c0: 0, r0: 0, c1: 2, r1: 0 }]);
  });

  it('returns an empty array for a formula with no references', () => {
    expect(extractFormulaRefRanges('=1+2*3')).toEqual([]);
  });

  it('returns an empty array for an empty string', () => {
    expect(extractFormulaRefRanges('')).toEqual([]);
  });
});

describe('isCellInAnyRefRange', () => {
  it('returns true for a coordinate inside a 1x1 range', () => {
    const ranges = extractFormulaRefRanges('=A1');
    expect(isCellInAnyRefRange(0, 0, ranges)).toBe(true);
  });

  it('returns false for a coordinate outside every range', () => {
    const ranges = extractFormulaRefRanges('=A1');
    expect(isCellInAnyRefRange(1, 1, ranges)).toBe(false);
  });

  it('returns true for a coordinate inside a multi-row range', () => {
    const ranges = extractFormulaRefRanges('=SUM(F1:F5)');
    expect(isCellInAnyRefRange(5, 2, ranges)).toBe(true);
  });

  it('returns false for a coordinate just past the end of a range', () => {
    const ranges = extractFormulaRefRanges('=SUM(F1:F5)');
    expect(isCellInAnyRefRange(5, 5, ranges)).toBe(false);
  });
});

describe('refRangeEdges', () => {
  it('marks all four edges true for a 1x1 range', () => {
    const ranges = extractFormulaRefRanges('=A1');
    expect(refRangeEdges(0, 0, ranges)).toEqual({ top: true, bottom: true, left: true, right: true });
  });

  it('marks only top/left/right for the first row of a multi-row single-column range', () => {
    const ranges = extractFormulaRefRanges('=SUM(F1:F5)');
    expect(refRangeEdges(5, 0, ranges)).toEqual({ top: true, bottom: false, left: true, right: true });
  });

  it('marks only bottom/left/right for the last row of a multi-row single-column range', () => {
    const ranges = extractFormulaRefRanges('=SUM(F1:F5)');
    expect(refRangeEdges(5, 4, ranges)).toEqual({ top: false, bottom: true, left: true, right: true });
  });

  it('marks no edges for a coordinate outside every range', () => {
    const ranges = extractFormulaRefRanges('=A1');
    expect(refRangeEdges(3, 3, ranges)).toEqual({ top: false, bottom: false, left: false, right: false });
  });
});
