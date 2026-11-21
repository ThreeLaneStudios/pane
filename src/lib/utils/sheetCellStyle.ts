// sheetCellStyle.ts — the WRITE path for the Sheets cell-styling feature
// (#2339 PR 15d-beta, `ShStylePanel`, scope report
// `docs/audits/2026-09-05-sheets-cell-styling-scope.md`). Pure, DOM-free,
// fully unit-tested — mirrors `sheetSelection.ts`'s own convention of
// keeping all the actual logic out of `SheetGrid.svelte`.
//
// `parseCellStyleJson` (sheetSnapshotEnvelope.ts) is the READ path — it
// tolerantly parses whatever `style_json` a cell already has, including
// both the legacy alpha-era single-object `bd` shape and this PR's new
// multi-border array shape. `buildCellStyleJson` here is the inverse: it
// takes structured style input and produces the JSON string that
// `parseCellStyleJson` (and the live grid's `cellInlineStyle`) can read
// back unchanged.
//
// Matt's outer-edge border ruling (2026-09-05, scope report "Matt's
// Rulings" §3): a "top" border applied to a multi-row range paints
// `border-top` on the TOP ROW's cells only (not every cell — that would
// draw a horizontal rule through the middle of the range). Same for
// bottom/left/right. "All" paints all four sides on every cell in the
// range. "None" clears every border in the range. This module doesn't
// itself decide per-cell borders (that requires the caller's selection
// range shape) — `SheetGrid.svelte`'s `applyStyleToSelection` computes
// which side(s), if any, apply to each coordinate and calls
// `mergeStylePatch` per cell; this module just merges/serializes.

import type { CellCoord, CellRange } from './sheetSelection';

/** A single border declaration for one side of a cell. `w` is 1/2/3px
 *  per the design's weight segments; `dashed` is optional (solid when
 *  omitted/false). */
export interface BorderSpec {
  side: 'top' | 'bottom' | 'left' | 'right';
  w: number;
  c: string;
  dashed?: boolean;
}

/** Structured style input for a single cell. `null` on any field means
 *  "clear this" (as opposed to `undefined`, which means "leave whatever
 *  was already there alone" — see `mergeStylePatch`). `bd` is always an
 *  ARRAY (one entry per styled side, per the beta multi-border format —
 *  a single cell can have e.g. both a top and a left border at once). */
export interface CellStyleInput {
  bg?: string | null;
  fg?: string | null;
  bd?: BorderSpec[] | null;
}

/** Builds the `style_json` string for a cell from structured input — the
 *  inverse of `parseCellStyleJson`. Returns `'{}'` when every field is
 *  null/undefined/empty (no styling), matching `parseCellStyleJson('{}')`
 *  → `{}`. Only fields that are actually set (non-null, non-undefined,
 *  and for `bd`, non-empty) are included in the output object — this
 *  keeps the JSON minimal and matches the existing alpha-era shape for
 *  bg/fg-only styles. */
export function buildCellStyleJson(style: CellStyleInput): string {
  const out: Record<string, unknown> = {};
  if (style.bg != null && style.bg !== '') out.bg = style.bg;
  if (style.fg != null && style.fg !== '') out.fg = style.fg;
  if (style.bd != null && style.bd.length > 0) out.bd = style.bd;
  return JSON.stringify(out);
}

/** Merges a partial style PATCH into an already-parsed existing style,
 *  producing the new full `CellStyleInput` to serialize.
 *
 *  Patch field semantics (per key):
 *    - `undefined` (key absent from the patch) → leave the existing
 *      value for that key untouched.
 *    - `null` → clear that key.
 *    - a concrete value → replace that key's value.
 *
 *  For `bd` specifically, the patch's border array REPLACES the
 *  existing one wholesale (never merged side-by-side) — each call to
 *  `applyStyleToSelection` in `SheetGrid.svelte` already computes the
 *  full resulting border set for a cell (e.g. "this cell keeps its old
 *  left border and gains a new top border" is resolved by the caller
 *  before calling this function), since only the caller has the range
 *  shape needed to know which side(s) apply to which coordinate. */
export function mergeStylePatch(existing: CellStyleInput, patch: Partial<CellStyleInput>): CellStyleInput {
  const merged: CellStyleInput = { ...existing };
  if ('bg' in patch) merged.bg = patch.bg;
  if ('fg' in patch) merged.fg = patch.fg;
  if ('bd' in patch) merged.bd = patch.bd;
  return merged;
}

/** Expands the current selection state into the list of cell coordinates
 *  an operation (style apply, style clear) should touch. Mirrors the
 *  same "range if present, else just the anchor" convention
 *  `clearSelectedContents`/`clearSelectedStyling` already use in
 *  `SheetGrid.svelte` for Delete/Backspace and "Clear styling" — this
 *  module gives that convention a single shared, unit-tested home rather
 *  than re-deriving it at each call site. Returns `[]` when there's no
 *  anchor (nothing selected). */
export function expandSelection(selection: { anchor: CellCoord | null; range: CellRange | null }): CellCoord[] {
  const { anchor, range } = selection;
  if (!anchor) return [];
  if (!range) return [anchor];
  const coords: CellCoord[] = [];
  for (let row = range.r0; row <= range.r1; row++) {
    for (let col = range.c0; col <= range.c1; col++) {
      coords.push({ row, col });
    }
  }
  return coords;
}

/** Raw, tolerant `style_json` → `CellStyleInput` parser used internally
 *  by `detectUnanimity`/`detectBorderUnanimity`/`applyStyleToSelection`.
 *  Deliberately separate from `parseCellStyleJson` (sheetSnapshotEnvelope.
 *  ts) — that function's job is producing ready-to-use CSS declaration
 *  strings for the live grid's `cellInlineStyle`; this one's job is
 *  producing structured, still-editable style DATA (raw colors, a real
 *  `BorderSpec[]`) for the style panel and for `mergeStylePatch` to merge
 *  a new patch into. Handles both the beta multi-border array `bd` shape
 *  and the legacy alpha-era single-object `bd` shape (wrapped into a
 *  one-entry array) — same backward-compat contract as
 *  `parseCellStyleJson`. Never throws; malformed/empty input returns
 *  `{}`. */
export function parseCellStyleRaw(styleJson: string | null | undefined): CellStyleInput {
  if (!styleJson) return {};
  let obj: unknown;
  try {
    obj = JSON.parse(styleJson);
  } catch {
    return {};
  }
  if (!obj || typeof obj !== 'object') return {};
  const raw = obj as Record<string, unknown>;
  const out: CellStyleInput = {};
  if (typeof raw.bg === 'string') out.bg = raw.bg;
  if (typeof raw.fg === 'string') out.fg = raw.fg;
  const isBorderSpec = (v: unknown): v is BorderSpec =>
    !!v && typeof v === 'object' &&
    typeof (v as Record<string, unknown>).side === 'string' &&
    typeof (v as Record<string, unknown>).w === 'number' &&
    typeof (v as Record<string, unknown>).c === 'string';
  const bd = raw.bd;
  if (Array.isArray(bd)) {
    const specs = bd.filter(isBorderSpec) as BorderSpec[];
    if (specs.length > 0) out.bd = specs;
  } else if (isBorderSpec(bd)) {
    out.bd = [bd];
  }
  return out;
}

/** Matt's outer-edge border ruling (scope report "Matt's Rulings" §3),
 *  expressed as a single pure per-cell resolver: given the side the user
 *  applied (`'top'|'bottom'|'left'|'right'|'all'|'none'`), a cell
 *  coordinate, and the active selection range, returns which physical
 *  border side(s) — if any — that ONE cell should have painted for this
 *  operation. `'top'/'bottom'/'left'/'right'` resolve to that single side
 *  only when the coordinate sits on the matching edge of `range` (e.g.
 *  `'top'` → `['top']` only for cells whose row equals `range.r0`, `[]`
 *  otherwise); `'all'` always resolves to all four sides; `'none'`
 *  always resolves to `[]`. This is the ONLY place
 *  `SheetGrid.svelte`'s `applyStyleToSelection` computes this — it calls
 *  this function once per selected cell rather than re-deriving the edge
 *  check inline, so this module (already the DOM-free, fully-unit-tested
 *  home for the rest of the WRITE-path logic) is the single source of
 *  truth for the ruling and the unit tests in `sheetCellStyle.test.ts`
 *  exercise the REAL logic the component runs, not a re-implementation of
 *  it. */
export function resolvedBorderSides(
  applied: BorderSpec['side'] | 'all' | 'none',
  coord: CellCoord,
  range: CellRange
): BorderSpec['side'][] {
  if (applied === 'none') return [];
  if (applied === 'all') return ['top', 'bottom', 'left', 'right'];
  if (applied === 'top') return coord.row === range.r0 ? ['top'] : [];
  if (applied === 'bottom') return coord.row === range.r1 ? ['bottom'] : [];
  if (applied === 'left') return coord.col === range.c0 ? ['left'] : [];
  if (applied === 'right') return coord.col === range.c1 ? ['right'] : [];
  return [];
}

/** Detects unanimity for a single scalar style key (`bg` or `fg`) across
 *  a set of cells' raw `style_json` strings. Matt's unanimity ruling
 *  (scope report §Matt's Rulings 4): the panel highlights a swatch only
 *  when EVERY cell in the selection agrees — otherwise nothing is
 *  highlighted (the standard indeterminate-checkbox convention). An
 *  empty cell list (no selection) returns `null`. Missing/unset values
 *  are normalized to `null` before comparing, so "no fill" (absent key,
 *  or empty/malformed style_json) counts as agreeing with an explicit
 *  unset value. */
export function detectUnanimity(cells: Array<{ style_json: string }>, key: 'bg' | 'fg'): string | null {
  if (cells.length === 0) return null;
  const values = cells.map((c) => parseCellStyleRaw(c.style_json)[key] ?? null);
  const first = values[0];
  for (const v of values) {
    if (v !== first) return null;
  }
  return first;
}

/** A single side's resolved border for `detectBorderUnanimity`'s
 *  `'unanimous'` state — `w`/`c`/`dashed` reflect the shared border
 *  entry every cell in the selection agreed on for that side. */
export interface BorderUnanimity {
  w: number;
  c: string;
  dashed: boolean;
}

/** #2874 item 45 — `detectBorderUnanimity`'s result for one side, as a
 *  real three-way discriminated union. Before this fix the function
 *  returned `BorderUnanimity | null`, and `null` was used for TWO
 *  semantically opposite situations, indistinguishably: "no cell in the
 *  selection has this side" and "cells disagree on this side" (including
 *  the partial-presence case — one cell bordered, one not). Callers had
 *  no way to tell those apart, which is exactly what let a genuinely
 *  mixed selection render as if it were unanimously borderless (see
 *  `docs/research/2026-09-24-scope-45-border-unanimity.md`).
 *
 *  - `'none'`     — no cell in the selection has a border on this side.
 *  - `'unanimous'` — every cell agrees (same `w`/`c`/`dashed`) on this
 *    side's border.
 *  - `'mixed'`    — cells disagree on this side: either some have it and
 *    some don't (partial presence), or every cell has it but `w`/`c`/
 *    `dashed` differ. Matt's ruling (#2874 item 45, 2026-09-24): partial
 *    presence counts as mixed, not absent — "one cell bordered, one
 *    not" is a disagreement, not a unanimous "no border". */
export type BorderSideResult = { state: 'none' } | ({ state: 'unanimous' } & BorderUnanimity) | { state: 'mixed' };

/** Border-specific unanimity: for the given `side`, reports whether the
 *  selection is unanimously borderless, unanimously bordered (with the
 *  shared `{w, c, dashed}`), or mixed — see `BorderSideResult`. Cells
 *  are passed by their raw `style_json` strings, same as
 *  `detectUnanimity`. An empty cell list is treated as `'none'` (no
 *  cells means nothing to disagree about — same convention
 *  `detectUnanimity` uses for an unset scalar). */
export function detectBorderUnanimity(cells: Array<{ style_json: string }>, side: BorderSpec['side']): BorderSideResult {
  if (cells.length === 0) return { state: 'none' };
  const entries = cells.map((c) => (parseCellStyleRaw(c.style_json).bd ?? []).find((b) => b.side === side) ?? null);
  const first = entries[0];
  for (const e of entries) {
    const same = (e === null && first === null) || (e !== null && first !== null && e.w === first.w && e.c === first.c && !!e.dashed === !!first.dashed);
    if (!same) return { state: 'mixed' };
  }
  if (!first) return { state: 'none' };
  return { state: 'unanimous', w: first.w, c: first.c, dashed: !!first.dashed };
}
