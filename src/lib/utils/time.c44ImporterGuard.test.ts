/**
 * time.c44ImporterGuard.test.ts — #3155 C44 (carry-over from #3154).
 *
 * Reed's own prescription, quoted in the scope brief verbatim: "a test
 * that the helper has importers would have caught this." A behavioral
 * test proves nothing for this item — `parseStoredTimestamp` and the
 * inline regex it replaces already do byte-identical date math (both
 * post-#3154), so running the same fixture methodology `time.test.ts`
 * uses would pass identically before AND after the wiring, proving
 * nothing about whether the wiring actually happened. The only test that
 * can prove this landed is structural, in the same style as this
 * codebase's existing source-text scanner pattern (`canvas.
 * showViewParity.test.ts`, `refreshEvent.mirrorParity.test.ts`): read the
 * real source files with `readFileSync` and assert on their literal text.
 *
 * Two assertions:
 * 1. `parseStoredTimestamp` has real importers in all three target files
 *    (`NotificationPanel.svelte`, `Settings.svelte`, `SessionRow.svelte`)
 *    — plus its own file (`formatRelativeTime`'s sibling use) and
 *    `time.test.ts` itself, so the import-statement regex has a known,
 *    non-zero floor even before counting the three targets.
 * 2. None of the three target files still contains a standalone
 *    `.replace(' ', 'T') + 'Z'` normalize literal OUTSIDE of
 *    `parseStoredTimestamp`'s own definition — i.e. no orphan
 *    reimplementation survives alongside the new import.
 *
 * Mutation: revert any ONE target file's import back to an inline
 * `.replace(' ', 'T') + 'Z'` reimplementation (what this fix replaced) —
 * either that file's import count drops out of the "real importer" set,
 * or the orphan-literal scan finds the reintroduced inline pattern (or
 * both) — reddens either way.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { join } from 'path';

const TARGET_FILES = [
  '../components/NotificationPanel.svelte',
  '../components/Settings.svelte',
  '../components/SessionRow.svelte',
] as const;

// This test file lives in `src/lib/utils/` — `__dirname` IS that
// directory, so `time.ts`/`time.test.ts` are read relative to it
// directly, and the three target `.svelte` files (in the sibling
// `components/` directory) via `../components/...`.
const LIB_UTILS_DIR = __dirname;

function readSource(relPath: string): string {
  return readFileSync(join(LIB_UTILS_DIR, relPath), 'utf-8');
}

// The exact inline normalize shape every one of the three sites used to
// reimplement, byte for byte (see the pre-fix doc comments this commit
// removes): `raw.replace(' ', 'T') + 'Z'` (optionally preceded by the
// variable name differing per site — matched generically on the
// `.replace(' ', 'T') + 'Z'` tail, which is the load-bearing, bug-prone
// part regardless of the receiver variable's name).
const ORPHAN_INLINE_NORMALIZE = /\.replace\(\s*['"] ['"]\s*,\s*['"]T['"]\s*\)\s*\+\s*['"]Z['"]/;

describe('C44 (#3155, carry-over from #3154) — parseStoredTimestamp wiring, structural guard', () => {
  it('parseStoredTimestamp has real importers: its own file + the three target files (sanity floor, non-zero)', () => {
    const timeTs = readSource('time.ts');
    const timeTestTs = readSource('time.test.ts');
    // Sanity: the extraction itself must find something, so an empty
    // match can never silently pass the "every target imports it" loop
    // below by both sides being vacuously zero.
    expect(timeTs).toContain('export function parseStoredTimestamp');
    expect(timeTestTs).toContain('parseStoredTimestamp');

    for (const target of TARGET_FILES) {
      const src = readSource(target);
      expect(
        /import\s*\{[^}]*\bparseStoredTimestamp\b[^}]*\}\s*from\s*['"]\$lib\/utils\/time['"]/.test(src),
        `${target} must import parseStoredTimestamp from $lib/utils/time`
      ).toBe(true);
      expect(
        src.includes('parseStoredTimestamp('),
        `${target} must actually CALL parseStoredTimestamp, not just import it unused`
      ).toBe(true);
    }
  });

  it('none of the three target files still contains an orphan inline .replace(\' \', \'T\') + \'Z\' normalize literal', () => {
    for (const target of TARGET_FILES) {
      const src = readSource(target);
      expect(
        ORPHAN_INLINE_NORMALIZE.test(src),
        `${target} must not contain a standalone inline normalize literal — the parse must be shared via parseStoredTimestamp, not reimplemented inline`
      ).toBe(false);
    }
  });

  it('time.ts itself (the ONE legitimate owner of this literal) still contains it exactly once — the sanity check for the regex above', () => {
    const timeTs = readSource('time.ts');
    const matches = timeTs.match(new RegExp(ORPHAN_INLINE_NORMALIZE.source, 'g')) ?? [];
    expect(matches.length, 'the regex must actually match something somewhere, or the three target-file checks above are vacuous').toBe(1);
  });
});
