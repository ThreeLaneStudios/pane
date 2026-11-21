// sheetCellCommit.test.ts — unit tests for the Sheets per-type value
// parsing/formatting helpers (#2339 PR 10, §2/§7/§8). Pure functions, no
// mocking — every test drives the real implementation.

import { describe, it, expect } from 'vitest';
import {
  parseDateMaskToIso,
  isoToDateMask,
  formatCurrencyDisplay,
  valueForCommit,
  toggleCheckboxValue,
} from './sheetCellCommit';

describe('parseDateMaskToIso', () => {
  it('parses a full MM/DD/YYYY mask to ISO', () => {
    expect(parseDateMaskToIso('08/04/2026')).toBe('2026-08-04');
  });

  it('parses a lenient M/D/YY mask to ISO, pivoting the 2-digit year', () => {
    expect(parseDateMaskToIso('8/4/26')).toBe('2026-08-04');
  });

  it('parses a 2-digit year >= 50 as 19xx', () => {
    expect(parseDateMaskToIso('1/1/99')).toBe('1999-01-01');
  });

  it('pads single-digit month/day to two digits in the ISO output', () => {
    expect(parseDateMaskToIso('1/2/2026')).toBe('2026-01-02');
  });

  it('rejects a month past 12', () => {
    expect(parseDateMaskToIso('13/01/2026')).toBeNull();
  });

  it('rejects a day past the month\'s max (calendar-invalid, not just >31)', () => {
    expect(parseDateMaskToIso('04/31/2026')).toBeNull();
  });

  it('rejects Feb 30 (calendar overflow that Date would otherwise silently normalize)', () => {
    expect(parseDateMaskToIso('02/30/2026')).toBeNull();
  });

  it('accepts Feb 29 on a leap year', () => {
    expect(parseDateMaskToIso('02/29/2028')).toBe('2028-02-29');
  });

  it('rejects Feb 29 on a non-leap year', () => {
    expect(parseDateMaskToIso('02/29/2026')).toBeNull();
  });

  it('rejects a non-numeric input', () => {
    expect(parseDateMaskToIso('not a date')).toBeNull();
  });

  it('rejects an ISO-shaped input (wrong mask direction)', () => {
    expect(parseDateMaskToIso('2026-08-04')).toBeNull();
  });

  it('rejects an empty string', () => {
    expect(parseDateMaskToIso('')).toBeNull();
  });

  it('tolerates surrounding whitespace', () => {
    expect(parseDateMaskToIso('  08/04/2026  ')).toBe('2026-08-04');
  });
});

describe('isoToDateMask', () => {
  it('converts an ISO date to MM/DD/YYYY', () => {
    expect(isoToDateMask('2026-08-04')).toBe('08/04/2026');
  });

  it('passes through unchanged when the input is not ISO-shaped', () => {
    expect(isoToDateMask('not-a-date')).toBe('not-a-date');
  });

  it('round-trips with parseDateMaskToIso', () => {
    const iso = parseDateMaskToIso('3/7/2026');
    expect(iso).not.toBeNull();
    expect(isoToDateMask(iso as string)).toBe('03/07/2026');
  });
});

describe('formatCurrencyDisplay', () => {
  it('formats a raw integer string to two decimal places with grouping', () => {
    expect(formatCurrencyDisplay('1400')).toBe('1,400.00');
  });

  it('formats a raw decimal string, rounding to two places', () => {
    expect(formatCurrencyDisplay('1400.5')).toBe('1,400.50');
  });

  it('formats a large number with thousands grouping', () => {
    expect(formatCurrencyDisplay('1234567')).toBe('1,234,567.00');
  });

  it('returns an empty string for an empty input (never throws)', () => {
    expect(formatCurrencyDisplay('')).toBe('');
  });

  it('passes through unchanged for a non-numeric string (mismatch-safe)', () => {
    expect(formatCurrencyDisplay('TBD')).toBe('TBD');
  });

  it('formats zero correctly', () => {
    expect(formatCurrencyDisplay('0')).toBe('0.00');
  });
});

describe('valueForCommit', () => {
  it('text: passthrough verbatim', () => {
    expect(valueForCommit('text', 'Longform essays')).toBe('Longform essays');
  });

  it('number: passthrough verbatim (no client-side numeric reformatting)', () => {
    expect(valueForCommit('number', '12')).toBe('12');
  });

  it('currency: passthrough verbatim — stores the raw typed digits, not a formatted string (ruling 3)', () => {
    expect(valueForCommit('currency', '1400')).toBe('1400');
  });

  it('select: passthrough verbatim', () => {
    expect(valueForCommit('select', 'In flight')).toBe('In flight');
  });

  it('date: converts a valid mask to ISO', () => {
    expect(valueForCommit('date', '08/04/2026')).toBe('2026-08-04');
  });

  it('date: stores the raw typed string verbatim when it doesn\'t parse (never blocks the write)', () => {
    expect(valueForCommit('date', 'next Tuesday')).toBe('next Tuesday');
  });

  it('a leading-= formula string passes through verbatim regardless of column type (backend validates on write)', () => {
    expect(valueForCommit('text', '=A1+B1')).toBe('=A1+B1');
    expect(valueForCommit('number', '=SUM(A1:A5)')).toBe('=SUM(A1:A5)');
  });
});

describe('toggleCheckboxValue', () => {
  it('toggles an empty/off value to "1"', () => {
    expect(toggleCheckboxValue('')).toBe('1');
  });

  it('toggles undefined to "1"', () => {
    expect(toggleCheckboxValue(undefined)).toBe('1');
  });

  it('toggles null to "1"', () => {
    expect(toggleCheckboxValue(null)).toBe('1');
  });

  it('toggles "0" to "1"', () => {
    expect(toggleCheckboxValue('0')).toBe('1');
  });

  it('toggles "1" to "0"', () => {
    expect(toggleCheckboxValue('1')).toBe('0');
  });

  it('toggles any other non-empty, non-"0" value to "0" (matches the read-render "is-on" predicate)', () => {
    expect(toggleCheckboxValue('true')).toBe('0');
  });
});
