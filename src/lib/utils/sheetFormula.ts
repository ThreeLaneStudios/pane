// sheetFormula.ts — pure formula-text helpers for the Sheets formula bar
// and in-cell formula editor (#2339 PR 14, §33.5). DOM-free, unit-tested,
// following the same pattern as sheetCellRender.ts / sheetCellCommit.ts.
//
// By the time formula text reaches the frontend it is ALREADY in A1
// display form — the backend's ReferenceMapper read path
// (`sheets.rs::translate_cells_for_display`, Matt ruling 2026-09-05 #8)
// re-renders every stored stable-ID reference back to its current A1
// position before the cell ever leaves `get_sheet_data`. This module
// therefore only ever needs to tokenize/scan A1-style formula text — it
// has no ID-form concerns and does zero translation of its own (the
// ReferenceMapper is a backend-only Rust concern, per the ruling; this
// file is display/highlight plumbing only).
//
// The tokenizer regex below is ported directly from
// `docs/design/sheets/sheets-shared.jsx`'s `Formula` component so the
// bar/in-cell editor's token boundaries match the locked design pixel-
// for-pixel (same split, same classification order).

export type FormulaTokenClass = 'sh-t-ref' | 'sh-t-fn' | 'sh-t-str' | 'sh-t-num' | 'sh-t-op';

export interface FormulaToken {
  text: string;
  cls: FormulaTokenClass;
}

// Matches: an A1 ref or range (`A1`, `B2:B9`), a function name immediately
// followed by `(` (`SUM(`, `IF(`), a quoted string literal, or a bare
// number. Everything else (operators, punctuation, whitespace) falls
// through as plain "op" text between matches.
const TOKEN_SPLIT_RE = /(\b[A-Z]+\d+(?::[A-Z]+\d+)?\b|\b[A-Z]{2,}(?=\()|"[^"]*"|\d+\.?\d*)/g;

/**
 * Tokenizes formula source text into colored spans, matching the design's
 * `.sh-t-ref` (cyan refs) / `.sh-t-fn` (full-strength function names) /
 * `.sh-t-str` (green strings — Matt ruling 2026-09-05 #1: a DELIBERATE
 * deviation from the design prose's "numbers and strings recede," which
 * the locked `sheets.css` itself contradicts by giving `.sh-t-str` the
 * success-green token; Matt ruled the CSS wins) / `.sh-t-num` (receding
 * numbers) / `.sh-t-op` (everything else — operators, punctuation,
 * function names' own parens, whitespace).
 */
export function tokenizeFormula(src: string): FormulaToken[] {
  const parts = String(src).split(TOKEN_SPLIT_RE);
  const tokens: FormulaToken[] = [];
  for (const p of parts) {
    if (!p) continue;
    let cls: FormulaTokenClass = 'sh-t-op';
    if (/^[A-Z]+\d+(:[A-Z]+\d+)?$/.test(p)) cls = 'sh-t-ref';
    else if (/^[A-Z]{2,}$/.test(p)) cls = 'sh-t-fn';
    else if (/^"/.test(p)) cls = 'sh-t-str';
    else if (/^\d/.test(p)) cls = 'sh-t-num';
    else if (/[a-z]/.test(p)) cls = 'sh-t-num';
    tokens.push({ text: p, cls });
  }
  return tokens;
}

/** A1 column letters (`"A"`, `"AB"`) -> zero-based index. Mirrors
 *  SheetGrid.svelte's own `colRef` inverse exactly (base-26, A=0). */
export function a1ColLettersToIndex(letters: string): number {
  let n = 0;
  for (const ch of letters.toUpperCase()) {
    n = n * 26 + (ch.charCodeAt(0) - 64);
  }
  return n - 1;
}

/** A 0-indexed, inclusive rectangular region of the REAL (sort_order)
 *  grid that a single formula reference/range denotes — a plain
 *  `CellRef` becomes a 1x1 region (`c0 === c1`, `r0 === r1`). */
export interface FormulaRefRange {
  c0: number;
  r0: number;
  c1: number;
  r1: number;
}

const REF_SCAN_RE = /\b([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?\b/g;

/**
 * Scans formula source text for every A1 ref/range it contains and
 * returns each as a 0-indexed rectangular region against the REAL
 * (sort_order) column/row grid — used to drive the dashed cyan ref
 * outline in the grid while a formula is being edited (design annotation:
 * "every range it references gets a dashed cyan outline… dashed, because
 * the solid outline already means 'this is your selection'").
 *
 * A bare function name like `SUM` never matches (no trailing digits), so
 * this never confuses a function call for a reference. Malformed/
 * unresolvable letter runs are skipped defensively rather than throwing —
 * this function is display-only and must never crash a live-typing
 * formula bar over a half-typed reference.
 */
export function extractFormulaRefRanges(src: string): FormulaRefRange[] {
  const ranges: FormulaRefRange[] = [];
  const re = new RegExp(REF_SCAN_RE.source, 'g');
  let m: RegExpExecArray | null;
  while ((m = re.exec(src)) !== null) {
    const [, startCol, startRow, endCol, endRow] = m;
    const c0 = a1ColLettersToIndex(startCol);
    const r0 = parseInt(startRow, 10) - 1;
    if (c0 < 0 || Number.isNaN(r0) || r0 < 0) continue;
    if (endCol && endRow) {
      const c1 = a1ColLettersToIndex(endCol);
      const r1 = parseInt(endRow, 10) - 1;
      if (c1 < 0 || Number.isNaN(r1) || r1 < 0) continue;
      ranges.push({ c0: Math.min(c0, c1), r0: Math.min(r0, r1), c1: Math.max(c0, c1), r1: Math.max(r0, r1) });
    } else {
      ranges.push({ c0, r0, c1: c0, r1: r0 });
    }
  }
  return ranges;
}

/** True iff the REAL (sort_order) coordinate `(col, row)` falls inside
 *  any of `ranges` — the per-cell predicate `SheetGrid.svelte` uses to
 *  decide whether a given real grid cell gets the `.sh-c.is-ref`
 *  dashed-outline treatment while a formula is being edited. */
export function isCellInAnyRefRange(col: number, row: number, ranges: FormulaRefRange[]): boolean {
  return ranges.some((r) => col >= r.c0 && col <= r.c1 && row >= r.r0 && row <= r.r1);
}

/** Which of the four dashed edges (`.sh-c.is-refedge-{t,b,l,r}`) a REAL
 *  `(col, row)` coordinate sits on, given it's already known to be inside
 *  at least one range (call after `isCellInAnyRefRange`, or accept a
 *  false-negative-safe "no edges" result for a coordinate outside every
 *  range — the edge checks below are trivially all-false in that case,
 *  matching "not in range -> no outline at all" from the caller). */
export function refRangeEdges(col: number, row: number, ranges: FormulaRefRange[]) {
  let top = false, bottom = false, left = false, right = false;
  for (const r of ranges) {
    if (col < r.c0 || col > r.c1 || row < r.r0 || row > r.r1) continue;
    if (row === r.r0) top = true;
    if (row === r.r1) bottom = true;
    if (col === r.c0) left = true;
    if (col === r.c1) right = true;
  }
  return { top, bottom, left, right };
}
