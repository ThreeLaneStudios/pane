/**
 * #2339 PR 15a — Board card face: sheet card compact-variant breakpoint
 * resolver.
 *
 * Pure-function unit tests for resolveSheetCardMode(), the single seam that
 * decides whether a sheet card renders its full micro-grid (glyph header +
 * 2 data rows) or the compact glyph-header-only strip, from the card's own
 * rendered width. See docs/design/sheets/README.md §4 / build constraint §1:
 * "below ~190px of card width the data rows drop and the card is glyph plus
 * header strip."
 */

import { describe, it, expect } from 'vitest';
import {
  resolveSheetCardMode,
  SHEET_CARD_COMPACT_THRESHOLD,
} from './sheetCardLayout';

describe('resolveSheetCardMode — #2339 PR 15a', () => {
  it('returns "full" at and above the compact threshold (190px)', () => {
    expect(resolveSheetCardMode(SHEET_CARD_COMPACT_THRESHOLD)).toBe('full');
    expect(resolveSheetCardMode(191)).toBe('full');
    expect(resolveSheetCardMode(280)).toBe('full');
    expect(resolveSheetCardMode(600)).toBe('full');
  });

  it('returns "compact" strictly below the threshold — boundary test', () => {
    expect(resolveSheetCardMode(SHEET_CARD_COMPACT_THRESHOLD - 1)).toBe('compact');
    expect(resolveSheetCardMode(189)).toBe('compact');
    expect(resolveSheetCardMode(176)).toBe('compact'); // the design's own narrow-card demo width
    expect(resolveSheetCardMode(0)).toBe('compact');
  });

  it('is a pure function — same input always yields the same output, no side effects', () => {
    const a = resolveSheetCardMode(200);
    const b = resolveSheetCardMode(200);
    expect(a).toBe(b);
    expect(a).toBe('full');
  });

  it('handles negative width defensively as "compact" (never throws)', () => {
    expect(resolveSheetCardMode(-50)).toBe('compact');
  });

  it('exposes the exact locked breakpoint constant', () => {
    expect(SHEET_CARD_COMPACT_THRESHOLD).toBe(190);
  });
});
