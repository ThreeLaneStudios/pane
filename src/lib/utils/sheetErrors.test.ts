// sheetErrors.test.ts — unit tests for the static error-code catalogue
// (#2339 PR 15b-alpha).

import { describe, it, expect } from 'vitest';
import { getErrorInfo } from './sheetErrors';

const KNOWN_CODES = ['#REF!', '#VALUE!', '#DIV/0!', '#NAME?', '#PARSE!', '#CYCLE!'];

describe('getErrorInfo', () => {
  it('returns a message + formula for every one of the six known codes', () => {
    for (const code of KNOWN_CODES) {
      const info = getErrorInfo(code);
      expect(info).not.toBeNull();
      expect(info!.code).toBe(code);
      expect(info!.message.length).toBeGreaterThan(0);
      expect(info!.formula.length).toBeGreaterThan(0);
    }
  });

  it('returns null for an unrecognized code', () => {
    expect(getErrorInfo('#WAT!')).toBeNull();
    expect(getErrorInfo('')).toBeNull();
  });

  it('#VALUE! matches the design catalogue verbatim (code, message, formula)', () => {
    expect(getErrorInfo('#VALUE!')).toEqual({
      code: '#VALUE!',
      message: 'A value in the formula is the wrong kind — usually text where a number is needed.',
      formula: '=D3*E3',
    });
  });

  it('no message names a specific cell (no culprit-cell naming, hard constraint)', () => {
    // The design's own illustrated #VALUE! popover mockup appends a
    // culprit clause ("B2 holds text") on top of the base ERRS message —
    // this catalogue must NOT carry that clause. A simple heuristic
    // guard: no message should contain a bare column-letter+row-number
    // token immediately followed by "holds" (the design's own phrasing).
    for (const code of KNOWN_CODES) {
      const info = getErrorInfo(code)!;
      expect(info.message).not.toMatch(/\b[A-Z]{1,2}\d+\s+holds\b/);
    }
  });

  it('every known code is distinct and every entry round-trips by its own code', () => {
    const codes = new Set(KNOWN_CODES.map((c) => getErrorInfo(c)!.code));
    expect(codes.size).toBe(KNOWN_CODES.length);
  });
});
