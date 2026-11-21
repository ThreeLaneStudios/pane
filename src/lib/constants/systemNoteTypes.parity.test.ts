/**
 * systemNoteTypes.parity.test.ts — #3029
 * (`~/Shared/ExampleApp/docs/audits/2026-10-12-scope-3029-system-duplicates.md`).
 *
 * `notes.rs`'s `SYSTEM_NOTE_TYPES` (`src-tauri/src/notes.rs`) is the
 * canonical Rust-side list of `notes.note_type` values that are
 * system-managed — `update_note_internal` rejects a title change on any
 * note whose `note_type` is in that list. `systemNoteTypes.ts`'s
 * `SYSTEM_NOTE_TYPES` is the single TypeScript-side mirror (imported by
 * `Editor.svelte` and its tests, not hand-copied by each). The two live
 * in different languages and processes — neither can `import`/`use` the
 * other's literal — so there is no way to make this truly one physical
 * source without either a runtime IPC round-trip (rejected: this data is
 * `const` and never changes at runtime, so IPC buys no freshness and
 * forces a loading state or a synchronous fallback array that defeats
 * the single-source goal) or a build-time codegen step (no such tooling
 * exists anywhere in this repo today).
 *
 * This follows the exact precedent set by
 * `ShHeaderMenu.columnTypesParity.test.ts` (#3026 / #2874 item 64, itself
 * following `src/lib/refreshEvent.mirrorParity.test.ts`, #2972 Stage 4):
 * read BOTH source files with `readFileSync`, extract the declared
 * values from the literal source text of each via regex, and compare
 * the two EXTRACTIONS against each other — never against a list this
 * test file restates from memory (the #2995 lesson).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const TS_PATH = resolve(__dirname, 'systemNoteTypes.ts');
const RS_PATH = resolve(__dirname, '../../../src-tauri/src/notes.rs');

const tsSource = readFileSync(TS_PATH, 'utf-8');
const rsSource = readFileSync(RS_PATH, 'utf-8');

/**
 * Extracts the ordered list of note-type ids from `notes.rs`'s
 * `pub const SYSTEM_NOTE_TYPES: &[&str] = &["a", "b", ...];` array
 * literal.
 */
function extractRsNoteTypeIds(source: string): string[] {
  const arrayMatch = source.match(
    /pub const SYSTEM_NOTE_TYPES:\s*&\[&str\]\s*=\s*&\[([^\]]*)\];/,
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
 * Extracts the ordered list of note-type ids from `systemNoteTypes.ts`'s
 * `export const SYSTEM_NOTE_TYPES = ['a', 'b', ...] as const;` array
 * literal.
 */
function extractTsNoteTypeIds(source: string): string[] {
  const arrayMatch = source.match(
    /export const SYSTEM_NOTE_TYPES = \[([\s\S]*?)\]\s*as const;/,
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /'([^']*)'/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

const rsIds = extractRsNoteTypeIds(rsSource);
const tsIds = extractTsNoteTypeIds(tsSource);

describe('notes.rs SYSTEM_NOTE_TYPES <-> systemNoteTypes.ts SYSTEM_NOTE_TYPES parity (#3029)', () => {
  it('both files were actually readable and non-trivially parsed (sanity — not a false pass from an empty regex match)', () => {
    expect(rsIds.length).toBeGreaterThan(0);
    expect(tsIds.length).toBeGreaterThan(0);
  });

  it('the set of ids matches between the Rust and TypeScript constants (a type present in only one file fails here)', () => {
    expect(new Set(tsIds)).toEqual(new Set(rsIds));
  });

  it('the order of ids matches between the Rust and TypeScript constants (a reorder in only one file fails here)', () => {
    expect(tsIds).toEqual(rsIds);
  });
});
