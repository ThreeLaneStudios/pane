import { describe, it, expect } from 'vitest';
import { formatRelativeTime, parseStoredTimestamp } from './time';

/**
 * #3151 PR3/C44 — `formatRelativeTime` parsed SQLite's naive-UTC
 * `datetime('now')` string ("YYYY-MM-DD HH:MM:SS", no `T`, no `Z`, no
 * offset) by handing it straight to `new Date(...)`. That shape is NOT
 * ISO-8601 — JS parses it as LOCAL wall-clock time. In any timezone
 * BEHIND UTC that makes `Date.now() - parsed` negative, which satisfies
 * `diffSec < 60` unconditionally: every timestamp showed "Just now"
 * forever, regardless of real age.
 *
 * **No `Date`/`Date.now` mocking anywhere in this file, deliberately.**
 * Two approaches were tried and rejected: (1) mutating `process.env.TZ`
 * mid-test does not retroactively change how Vitest's jsdom worker pool
 * parses date strings once `Date`/`Intl` has already been touched once
 * in that worker — confirmed empirically, not assumed; (2) `vi.
 * useFakeTimers()` replaces `Date` with an implementation whose string
 * parser treats a NAIVE (no `Z`) string as UTC unconditionally,
 * regardless of ambient `TZ` — which would mask the exact bug this
 * suite exists to catch (the real engine's naive-string parse is
 * LOCAL-time, not UTC). Instead, every test below builds its "N seconds
 * ago" fixture from the REAL `Date.now()` at test time and compares
 * against the real current instant — no fixed clock, no mocked
 * `Date.now`, so the only thing that can make `parseStoredTimestamp`'s
 * output diverge from "the real elapsed time" is the parse itself
 * mis-handling the ambient timezone — which is exactly the bug.
 *
 * **Why this still needs a non-UTC, real process-level `TZ` to actually
 * prove the regression (the hard rule: "test under a non-UTC TZ
 * explicitly" — C44 is timezone code, so that risk is maximal):** this
 * bug is invisible under `TZ=UTC` by its own nature (parsing a naive
 * string as "local" is a no-op when local IS UTC). This file's own
 * default-pool run (`TZ=UTC`, matching every other frontend suite in
 * this repo) therefore passes with or without the fix — it is run here
 * for coverage and the `parseStoredTimestamp` unit tests below (which
 * ARE TZ-anchor-independent, via `Date.UTC`), but the PR report's own
 * mutation proof additionally runs this exact file under a real,
 * separate `TZ=America/New_York npx vitest run ...` process invocation,
 * confirmed green with the fix and RED (reverted to the bare
 * `new Date(timestamp)` call) under that non-UTC TZ specifically.
 */
describe('parseStoredTimestamp — TZ-independent parsing of SQLite UTC strings (C44)', () => {
  it('a bare SQLite datetime string parses to the UTC instant it names, not a local-time reinterpretation', () => {
    const parsed = parseStoredTimestamp('2026-11-03 11:58:30');
    expect(parsed.getTime()).toBe(Date.UTC(2026, 10, 3, 11, 58, 30));
  });

  it('an already-Z-suffixed ISO string passes through unmodified (no double-Z)', () => {
    const parsed = parseStoredTimestamp('2026-11-03T11:58:30Z');
    expect(parsed.getTime()).toBe(Date.UTC(2026, 10, 3, 11, 58, 30));
  });

  it('an already-offset ISO string passes through unmodified', () => {
    const parsed = parseStoredTimestamp('2026-11-03T06:58:30-05:00');
    expect(parsed.getTime()).toBe(Date.UTC(2026, 10, 3, 11, 58, 30));
  });

  it('a bare epoch number passes through unchanged', () => {
    const epoch = Date.UTC(2026, 10, 3, 11, 58, 30);
    expect(parseStoredTimestamp(epoch).getTime()).toBe(epoch);
  });
});

/** Builds the exact "YYYY-MM-DD HH:MM:SS" shape SQLite's `datetime('now')`
 *  produces, in UTC, for a real instant `secondsAgo` seconds before the
 *  real "now" at call time — deliberately NOT a fixed/mocked clock. */
function sqliteUtcSecondsAgo(secondsAgo: number): string {
  return new Date(Date.now() - secondsAgo * 1000)
    .toISOString()
    .slice(0, 19)
    .replace('T', ' ');
}

describe('formatRelativeTime — a SQLite timestamp a few minutes old is never stuck at "Just now" (C44)', () => {
  it('150 seconds old reads "2 minutes ago", not "Just now"', () => {
    const result = formatRelativeTime(sqliteUtcSecondsAgo(150));
    expect(result).not.toBe('Just now');
    expect(result).not.toContain('NaN');
    expect(result).toBe('2 minutes ago');
  });

  it('right now still reads "Just now" (regression guard — not now permanently wrong the other way)', () => {
    expect(formatRelativeTime(sqliteUtcSecondsAgo(1))).toBe('Just now');
  });

  it('3 hours and 30 seconds old reads "3 hours ago", never negative or NaN', () => {
    const result = formatRelativeTime(sqliteUtcSecondsAgo(3 * 3600 + 30));
    expect(result).not.toContain('NaN');
    expect(result).not.toMatch(/^-/);
    expect(result).toBe('3 hours ago');
  });

  it('null/undefined/empty still return "Never" (unchanged, regression guard)', () => {
    expect(formatRelativeTime(null)).toBe('Never');
    expect(formatRelativeTime(undefined)).toBe('Never');
    expect(formatRelativeTime('')).toBe('Never');
  });
});
