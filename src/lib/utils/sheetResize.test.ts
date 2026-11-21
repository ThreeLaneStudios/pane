// sheetResize.test.ts — unit tests for the Sheets column/row resize
// hit-zone/size math (#2339 PR 13, ShResize, §33.3.3).

import { describe, it, expect } from 'vitest';
import {
  detectColumnGrip,
  detectRowGrip,
  computeNewSize,
  formatReadout,
  MIN_COLUMN_WIDTH,
  MIN_ROW_HEIGHT,
} from './sheetResize';

describe('detectColumnGrip', () => {
  it('returns the boundary index when the cursor is exactly on the boundary', () => {
    expect(detectColumnGrip(100, [50, 100, 200])).toBe(1);
  });

  it('returns the boundary index when the cursor is within 3px of the boundary', () => {
    expect(detectColumnGrip(103, [50, 100, 200])).toBe(1);
    expect(detectColumnGrip(97, [50, 100, 200])).toBe(1);
  });

  it('returns null when the cursor is outside the 6px zone (more than 3px away)', () => {
    expect(detectColumnGrip(104, [50, 100, 200])).toBeNull();
    expect(detectColumnGrip(96, [50, 100, 200])).toBeNull();
  });

  it('returns null for an empty boundaries array', () => {
    expect(detectColumnGrip(100, [])).toBeNull();
  });

  it('picks the closest boundary when multiple boundaries are within range of each other', () => {
    // Boundaries 4px apart (narrower than the 6px zone) — cursor closer to
    // the second boundary.
    expect(detectColumnGrip(53, [50, 54])).toBe(1);
    // Cursor closer to the first.
    expect(detectColumnGrip(51, [50, 54])).toBe(0);
  });

  it('ties break toward the lower index', () => {
    // Cursor exactly equidistant between two boundaries.
    expect(detectColumnGrip(52, [50, 54, 999])).toBe(0);
  });
});

describe('detectRowGrip', () => {
  it('returns the boundary index when the cursor is within the 6px zone', () => {
    expect(detectRowGrip(200, [34, 68, 200])).toBe(2);
    expect(detectRowGrip(203, [34, 68, 200])).toBe(2);
    expect(detectRowGrip(197, [34, 68, 200])).toBe(2);
  });

  it('returns null when the cursor is outside every boundary\'s zone', () => {
    expect(detectRowGrip(50, [34, 68, 200])).toBeNull();
  });

  it('returns null for an empty boundaries array', () => {
    expect(detectRowGrip(34, [])).toBeNull();
  });
});

describe('computeNewSize', () => {
  it('applies a positive delta to the start size', () => {
    expect(computeNewSize(148, 20, MIN_COLUMN_WIDTH)).toBe(168);
  });

  it('applies a negative delta to the start size', () => {
    expect(computeNewSize(148, -20, MIN_COLUMN_WIDTH)).toBe(128);
  });

  it('returns the start size unchanged for a zero delta', () => {
    expect(computeNewSize(148, 0, MIN_COLUMN_WIDTH)).toBe(148);
    expect(computeNewSize(34, 0, MIN_ROW_HEIGHT)).toBe(34);
  });

  it('clamps at the column floor (40) when the delta would go below it', () => {
    expect(computeNewSize(50, -100, MIN_COLUMN_WIDTH)).toBe(MIN_COLUMN_WIDTH);
  });

  it('clamps at the row floor (24) when the delta would go below it', () => {
    expect(computeNewSize(34, -100, MIN_ROW_HEIGHT)).toBe(MIN_ROW_HEIGHT);
  });

  it('does not clamp when the result is exactly at the floor', () => {
    expect(computeNewSize(60, -20, MIN_COLUMN_WIDTH)).toBe(MIN_COLUMN_WIDTH);
  });

  it('does not clamp when the result is comfortably above the floor', () => {
    expect(computeNewSize(200, 50, MIN_COLUMN_WIDTH)).toBe(250);
  });
});

describe('formatReadout', () => {
  it('formats a size as "<n> px"', () => {
    expect(formatReadout(124)).toBe('124 px');
  });

  it('formats the column floor value', () => {
    expect(formatReadout(40)).toBe('40 px');
  });

  it('formats the row floor value', () => {
    expect(formatReadout(24)).toBe('24 px');
  });

  it('rounds a non-integer input to the nearest whole pixel', () => {
    expect(formatReadout(124.4)).toBe('124 px');
    expect(formatReadout(124.6)).toBe('125 px');
  });

  it('has a space before the unit, unit is lowercase "px"', () => {
    const text = formatReadout(62);
    expect(text).toBe('62 px');
    expect(text).toMatch(/^\d+ px$/);
  });
});
