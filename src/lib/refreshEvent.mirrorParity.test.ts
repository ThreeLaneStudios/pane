/**
 * refreshEvent.mirrorParity.test.ts — #2972 Stage 4, Reed's test gap
 * (`docs/spec/data-refresh.md` §34.1: "Both backend (`refresh_event.rs`)
 * and app (`refreshEvent.ts`) mirrors gain the `ENTITY_CATEGORY` constant
 * together... a drift between the two mirrors means events silently
 * never match.").
 *
 * Reed's gap: every existing test (`refreshEvent.test.ts`'s "identical
 * shape, both senders" assertion, every per-tool backend emit test, every
 * per-screen listener test) exercises ONE side's constants against a
 * literal string it already knows is correct — none of them read BOTH
 * `refresh_event.rs` and `refreshEvent.ts` and compare what's actually
 * declared in each. A future PR could add a sixth `entity_type` to one
 * file and forget the other, and every existing test would stay green
 * (because each only ever checks its own side against a hardcoded
 * string) while the two mirrors silently drift — the exact failure mode
 * this event design depends on never happening (§34.1: "a drift between
 * the two means events silently never match").
 *
 * This test reads BOTH source files directly (via `fs.readFileSync`,
 * parsed with a regex, not re-typed by hand) and asserts the two
 * extracted sets are identical — it does not restate either file's
 * contents as its own expectation, so a genuine drift in EITHER file
 * fails this test, not just a drift away from a value this file
 * hardcodes.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const TS_PATH = resolve(__dirname, 'refreshEvent.ts');
const RS_PATH = resolve(__dirname, '../../src-tauri/src/refresh_event.rs');

const tsSource = readFileSync(TS_PATH, 'utf-8');
const rsSource = readFileSync(RS_PATH, 'utf-8');

/** Extracts `export const NAME = 'value' as const;` pairs from the TS mirror. */
function extractTsConstants(source: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /export const ([A-Z_]+) = '([^']*)' as const;/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source)) !== null) {
    out[m[1]] = m[2];
  }
  return out;
}

/** Extracts `pub const NAME: &str = "value";` pairs from the Rust mirror. */
function extractRsConstants(source: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /pub const ([A-Z_]+): &str = "([^"]*)";/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source)) !== null) {
    out[m[1]] = m[2];
  }
  return out;
}

const tsConstants = extractTsConstants(tsSource);
const rsConstants = extractRsConstants(rsSource);

describe('refresh_event.rs <-> refreshEvent.ts mirror parity (#2972 stage 4, Reed\'s gap)', () => {
  it('both files were actually readable and non-trivially parsed (sanity — not a false pass from an empty regex match)', () => {
    expect(Object.keys(tsConstants).length).toBeGreaterThan(0);
    expect(Object.keys(rsConstants).length).toBeGreaterThan(0);
  });

  it('the event name constant (EVENT_DATA_CHANGED) matches between the two files', () => {
    expect(tsConstants.EVENT_DATA_CHANGED).toBeDefined();
    expect(rsConstants.EVENT_DATA_CHANGED).toBeDefined();
    expect(tsConstants.EVENT_DATA_CHANGED).toBe(rsConstants.EVENT_DATA_CHANGED);
  });

  it('every ENTITY_* constant name declared in EITHER file is declared with the SAME value in the other', () => {
    const tsEntityNames = Object.keys(tsConstants).filter((k) => k.startsWith('ENTITY_')).sort();
    const rsEntityNames = Object.keys(rsConstants).filter((k) => k.startsWith('ENTITY_')).sort();

    // Same SET of entity_type constant names on both sides — a constant
    // added to only one file (the literal drift this test exists to
    // catch) fails here.
    expect(tsEntityNames).toEqual(rsEntityNames);
    expect(tsEntityNames.length).toBe(5); // note, sheet, folder, project, category

    for (const name of tsEntityNames) {
      expect(tsConstants[name], `${name} value must match between refreshEvent.ts and refresh_event.rs`).toBe(rsConstants[name]);
    }
  });

  it('every REASON_* constant name declared in EITHER file is declared with the SAME value in the other', () => {
    const tsReasonNames = Object.keys(tsConstants).filter((k) => k.startsWith('REASON_')).sort();
    const rsReasonNames = Object.keys(rsConstants).filter((k) => k.startsWith('REASON_')).sort();

    expect(tsReasonNames).toEqual(rsReasonNames);
    expect(tsReasonNames.length).toBe(4); // created, edited, moved, deleted — no fifth (§34.1)

    for (const name of tsReasonNames) {
      expect(tsConstants[name], `${name} value must match between refreshEvent.ts and refresh_event.rs`).toBe(rsConstants[name]);
    }
  });

  it('entity_id for category is documented as the category name itself in BOTH files (no surrogate id column)', () => {
    // Not a constant — a documentation/contract assertion read from both
    // source files' own comments, so a future edit that silently changes
    // this contract in only one file is caught here too.
    expect(tsSource).toMatch(/entity_id.*for.*category.*is the category name/i);
    expect(rsSource).toMatch(/entity_id.*for.*`?category`?.*is the category name/i);
  });
});
