/**
 * #2339 PR 15c — New Sheet button polish: header-width breakpoint resolver.
 *
 * Pure-function unit tests for resolveHeaderButtonMode(), the single seam that
 * decides the three-step New Sheet/New Note collapse (full/icon/menu) from a
 * host header's own rendered width. See docs/design/sheets/NOTES.md §A /
 * README.md "Addendum rulings" #7 for the locked breakpoints:
 *   >= 900px        both labelled ("full")
 *   720px – 900px   New Sheet icon-only ("icon")
 *   < 720px         New Sheet folds into a caret menu ("menu")
 */

import { describe, it, expect } from 'vitest';
import {
  resolveHeaderButtonMode,
  HEADER_WIDE_THRESHOLD,
  HEADER_NARROW_THRESHOLD,
} from './newSheetButtonLayout';

describe('resolveHeaderButtonMode — #2339 PR 15c', () => {
  it('returns "full" at and above the wide threshold (900px)', () => {
    expect(resolveHeaderButtonMode(HEADER_WIDE_THRESHOLD)).toBe('full');
    expect(resolveHeaderButtonMode(1440)).toBe('full');
    expect(resolveHeaderButtonMode(901)).toBe('full');
  });

  it('returns "icon" strictly between the narrow and wide thresholds, inclusive of the narrow edge', () => {
    expect(resolveHeaderButtonMode(HEADER_NARROW_THRESHOLD)).toBe('icon');
    expect(resolveHeaderButtonMode(820)).toBe('icon');
    expect(resolveHeaderButtonMode(HEADER_WIDE_THRESHOLD - 1)).toBe('icon');
  });

  it('returns "menu" strictly below the narrow threshold (720px)', () => {
    expect(resolveHeaderButtonMode(HEADER_NARROW_THRESHOLD - 1)).toBe('menu');
    expect(resolveHeaderButtonMode(560)).toBe('menu');
    expect(resolveHeaderButtonMode(0)).toBe('menu');
  });

  it('is a pure function — same input always yields the same output, no side effects', () => {
    const a = resolveHeaderButtonMode(750);
    const b = resolveHeaderButtonMode(750);
    expect(a).toBe(b);
    expect(a).toBe('icon');
  });

  it('handles negative width defensively as "menu" (never throws)', () => {
    expect(resolveHeaderButtonMode(-100)).toBe('menu');
  });

  it('exposes the exact locked breakpoint constants', () => {
    expect(HEADER_WIDE_THRESHOLD).toBe(900);
    expect(HEADER_NARROW_THRESHOLD).toBe(720);
  });
});
