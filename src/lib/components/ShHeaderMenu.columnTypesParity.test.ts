/**
 * ShHeaderMenu.columnTypesParity.test.ts — #2874 item 64
 * (`docs/audits/2026-10-12-scope-2874-item64-column-types.md`).
 *
 * `sheets.rs`'s `VALID_COLUMN_TYPES` (`src-tauri/src/sheets.rs:65-66`) is
 * the canonical list of `sheet_columns.col_type` values. Item 62 (#3002)
 * made every Rust-side consumer read that one constant instead of
 * hand-typing a copy. `ShHeaderMenu.svelte`'s `TYPES` array (:131-137) —
 * which drives the type-change dropdown and the "current type" badge
 * label — is a UI-side hand-typed mirror that was never wired to that
 * fix, because it lives in a different language and can't `use` a Rust
 * const. A 7th type added to `VALID_COLUMN_TYPES` with no corresponding
 * `TYPES` entry would silently never appear as a selectable option in
 * the dropdown, and nothing before this test would catch that.
 *
 * This follows the exact precedent set by
 * `src/lib/refreshEvent.mirrorParity.test.ts` (#2972 Stage 4): read BOTH
 * source files with `readFileSync`, extract the declared values from the
 * literal source text of each via regex, and compare the two
 * EXTRACTIONS against each other — never against a list this test file
 * restates from memory. A test that hardcodes
 * `expect(ids).toEqual(['text','number','currency','date','checkbox','select'])`
 * would prove the test agrees with itself, not that `ShHeaderMenu.svelte`
 * agrees with `sheets.rs` (the #2995 lesson, cited explicitly in the
 * scope doc's §5).
 *
 * Mechanism/label/order decisions are per the scope doc's §2 (parity
 * test, not a Tauri command or build-time codegen), §3 (labels stay in
 * TS, the parity test is what closes the real gap), and §4 (order is
 * part of the contract, not just set membership).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SVELTE_PATH = resolve(__dirname, 'ShHeaderMenu.svelte');
const RS_PATH = resolve(__dirname, '../../../src-tauri/src/sheets.rs');

const svelteSource = readFileSync(SVELTE_PATH, 'utf-8');
const rsSource = readFileSync(RS_PATH, 'utf-8');

/**
 * Extracts the ordered list of column-type ids from `sheets.rs`'s
 * `VALID_COLUMN_TYPES: [&str; N] = ["a", "b", ...];` array literal.
 */
function extractRsColumnTypeIds(source: string): string[] {
  const arrayMatch = source.match(
    /const VALID_COLUMN_TYPES:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\]/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /"([^"]*)"/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

/**
 * Extracts the ordered list of `[id, label]` tuples from
 * `ShHeaderMenu.svelte`'s `const TYPES: Array<[string, string]> = [...]`
 * array literal.
 */
function extractSvelteColumnTypes(source: string): Array<[string, string]> {
  const arrayMatch = source.match(
    /const TYPES:\s*Array<\[string,\s*string\]>\s*=\s*\[([\s\S]*?)\];/,
  );
  if (!arrayMatch) return [];
  const pairs: Array<[string, string]> = [];
  const itemRe = /\[\s*'([^']*)'\s*,\s*'([^']*)'\s*\]/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    pairs.push([m[1], m[2]]);
  }
  return pairs;
}

const rsIds = extractRsColumnTypeIds(rsSource);
const sveltePairs = extractSvelteColumnTypes(svelteSource);
const svelteIds = sveltePairs.map(([id]) => id);

describe('sheets.rs VALID_COLUMN_TYPES <-> ShHeaderMenu.svelte TYPES parity (#2874 item 64)', () => {
  it('both files were actually readable and non-trivially parsed (sanity — not a false pass from an empty regex match)', () => {
    expect(rsIds.length).toBeGreaterThan(0);
    expect(svelteIds.length).toBeGreaterThan(0);
  });

  it('the set of ids matches between VALID_COLUMN_TYPES and TYPES (a type present in only one file fails here)', () => {
    expect(new Set(svelteIds)).toEqual(new Set(rsIds));
  });

  it('the order of ids matches between VALID_COLUMN_TYPES and TYPES (a reorder in only one file fails here)', () => {
    expect(svelteIds).toEqual(rsIds);
  });

  it('every id in TYPES has a non-empty display label', () => {
    for (const [id, label] of sveltePairs) {
      expect(label.trim().length, `id "${id}" must have a non-empty label`).toBeGreaterThan(0);
    }
  });
});
