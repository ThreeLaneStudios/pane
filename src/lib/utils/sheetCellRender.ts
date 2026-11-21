// sheetCellRender.ts — pure cell-kind classification and display-value
// helpers for the Sheets grid (#2339 PR 10, §5.3/§7). Extracted out of
// SheetGrid.svelte's PR 9 inline functions (`cellKind`/`displayValue`/
// `cellClass`) so they're unit-testable in isolation, following the same
// pattern PR 9 already established for `sheetSelection.ts`/
// `sheetGridPivot.ts` — pure, DOM-free, no Svelte dependency.
//
// Carries the PR 10 `.sh-c--raw` fix (scope report §5.3): a formula cell
// (`raw_value` starts with `=`) with `computed_value == null` now renders
// as the `'raw'` kind (mono font, `fg-3`, NO corner wedge) instead of
// `'calc'` (which implies "trustworthy computed value," wrongly, for a
// cell whose computed value hasn't actually arrived yet — a race window
// that's inert in PR 9's read-only build but live on every single formula
// commit once PR 10's optimistic-update-then-refetch flow exists).

import type { SheetCell } from '$lib/types';
import { isInRange, isAnchor, rangeEdges, type CellCoord, type CellRange } from './sheetSelection';
import { isTypeMismatch } from './sheetView';
import { parseCellStyleJson } from './sheetSnapshotEnvelope';

export type CellKind = 'normal' | 'calc' | 'err' | 'raw' | 'mis';

/** Monospace glyph for a column's type mark (T/#/$ for text/number/
 *  currency, D for date, a checkbox glyph, and a
 *  generic mark for select). Text-only — no SVG dependency.
 *
 *  #2339 PR 15b-beta: extracted verbatim out of `SheetGrid.svelte`'s
 *  own private `typeGlyph()` (PR 9) so the new sheet-native version-
 *  history preview grid (`ShSnapshotPreview.svelte`, which is
 *  deliberately its OWN lightweight renderer, not a SheetGrid instance)
 *  can render identical header glyphs without duplicating the
 *  switch-statement. `SheetGrid.svelte` now imports this instead of
 *  keeping its own copy — zero behavior change, same six cases, same
 *  fallback. */
export function typeGlyph(colType: string): string {
  switch (colType) {
    case 'text': return 'T';
    case 'number': return '#';
    case 'currency': return '$';
    case 'date': return 'D';
    case 'checkbox': return '☑';
    case 'select': return '≡';
    default: return '';
  }
}

/**
 * Classifies a cell's rendering kind. `colType` was reserved (unused) by
 * PR 10 for this exact purpose (Matt's ruling 6, deferred there) — PR 11
 * (#2339, ShMismatch, Matt's ruling 2026-09-04 "ShMismatch is IN") wires it
 * through to `isTypeMismatch` (sheetView.ts) for the new `'mis'` branch.
 *
 * `choices` is only consulted for a `select` column (the mismatch
 * classifier's own `select`-specific check) — omitted by every non-select
 * call site, harmless (defaults to "no valid choices configured") for one
 * that doesn't pass it.
 *
 * Check order (deliberate): formula-cell branches resolve FIRST — a
 * formula's raw text (`=SUM(...)`) is never itself run through the
 * mismatch classifier (it would trivially "mismatch" against every
 * column type, which is meaningless — a formula cell's `kind` is entirely
 * about its computed result, never its own literal text). Mismatch is
 * checked only for a genuine plain (non-formula) value, after the
 * formula/normal split.
 */
export function cellKind(cell: SheetCell | null, colType: string, choices?: string[]): CellKind {
  if (!cell) return 'normal';
  const raw = cell.raw_value ?? '';
  const isFormula = raw.startsWith('=');
  if (isFormula) {
    const computed = cell.computed_value;
    if (computed != null && computed.startsWith('#')) return 'err';
    if (computed != null) return 'calc';
    // Formula stored but no computed_value came back — either the
    // optimistic local patch from a just-committed formula edit (PR 10's
    // own update_sheet_cell return shape, which never populates
    // computed_value — sheets.rs:1327), a guard-rail skip (500+ formula
    // cells), or a sync-arrived formula not yet recomputed. Render as raw
    // text, not a trustworthy computed value (#2339 PR 10, §5.3).
    return 'raw';
  }
  if (isTypeMismatch(raw, colType, choices)) return 'mis';
  return 'normal';
}

/** The text to render inside a cell — computed value if present (formula
 *  cells that resolved), otherwise the raw stored value. */
export function displayValue(cell: SheetCell | null): string {
  if (!cell) return '';
  if (cell.computed_value != null) return cell.computed_value;
  return cell.raw_value ?? '';
}

/** Extracts a currency column's ISO code from its `options_json`
 *  (`{ "currency": "USD" }` per docs/spec/sheets.md §33.3.2), falling
 *  back to `'USD'` for empty/malformed/missing payloads.
 *
 *  #2874 item 38: consolidated here (single source of truth) out of what
 *  were previously TWO independent, near-identical copies —
 *  `SheetGrid.svelte`'s own private `currencyCode()` (checked
 *  `opts.currency || opts.code`) and `ShSnapshotPreview.svelte`'s own
 *  private `currencyCode()` (which read the WRONG key, `currency_code`,
 *  never matching the spec's actual `{ "currency": ... }` shape — a
 *  latent bug this consolidation also fixes for free, since the preview
 *  now shares the live grid's exact lookup). Both components import this
 *  one function instead of keeping their own copy — Matt's ruling
 *  2026-09-15 ("same formatter", item 38 ruling 3). */
export function currencyCode(optionsJson: string): string {
  try {
    const opts = JSON.parse(optionsJson || '{}');
    return opts.currency || opts.code || 'USD';
  } catch {
    return 'USD';
  }
}

/** Formats a currency cell's raw/computed stored value (a decimal
 *  string, per the canonical storage format in docs/spec/sheets.md
 *  §33.3.1) into a locale-correct display string with the currency's
 *  symbol and exactly two decimal places, e.g. `'930'` -> `'$930.00'`,
 *  `'930.5'` -> `'$930.50'`.
 *
 *  #2874 item 38 (Matt's ruling 2026-09-15, ruling 1): uses
 *  `Intl.NumberFormat('en-US', { style: 'currency', ... })` — the
 *  standard, locale-correct approach that handles symbol lookup,
 *  thousands separators, and 2-decimal rounding/padding in one call,
 *  replacing the old raw-decimal + bare-ISO-code (`"930 USD"`) rendering.
 *
 *  Non-numeric/unparseable raw values (including the empty string, and
 *  error strings like `'#DIV/0!'` that a formula-cell's computed_value
 *  can legitimately hold) fall back to returning the raw value UNCHANGED
 *  — currency cells with a genuinely unparseable value already render
 *  via the separate `'mis'`-kind branch upstream (`cellKind()`) in the
 *  live grid, so this only needs a graceful no-op fallback for the
 *  handful of non-`'mis'` callers (e.g. `ShSnapshotPreview.svelte`'s
 *  frozen historical cells, which never run the mismatch classifier at
 *  all) that could still hand it something non-numeric. */
export function formatCurrencyValue(rawValue: string, optionsJson: string): string {
  if (rawValue === '' || rawValue == null) return rawValue ?? '';
  const parsed = Number(rawValue);
  if (Number.isNaN(parsed)) return rawValue;
  const code = currencyCode(optionsJson);
  try {
    return new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: code,
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    }).format(parsed);
  } catch {
    return rawValue;
  }
}

/** Builds the inline `style` attribute string for a real grid cell's
 *  `<div>` (#2339 PR 15b-alpha (alpha), live-grid rendering of
 *  `style_json`, scope report §3.4). Wraps `parseCellStyleJson`
 *  (sheetSnapshotEnvelope.ts) — the same tolerant bg/fg/border parser the
 *  read-only snapshot preview already uses — but adds the ONE rule that
 *  parser has no opinion on: which of the parsed declarations are safe to
 *  apply to a LIVE (non-frozen) cell whose `kind` might be `'err'`.
 *
 *  CRITICAL, dedicated-test-covered carve-out (Matt's hard requirement,
 *  scope report §3.4/§6): an inline `style` attribute has higher CSS
 *  specificity than any class rule, including `.sh-c--err { color:
 *  var(--status-danger) }` (SheetGrid.svelte). Applying a user's chosen
 *  ink (`fg`) unconditionally would silently override the error color and
 *  break the design's explicit "error text always wins over any fill"
 *  contrast guarantee. So:
 *    - `bg` (background/fill)  → ALWAYS applied, any kind.
 *    - `bd` (border, any side) → ALWAYS applied, any kind.
 *    - `fg` (ink/text color)   → applied ONLY when `kind !== 'err'`.
 *  Fills/borders sit "behind" the value and never touch text-color
 *  contrast, so they're safe to apply unconditionally (matches
 *  `ShCoexist`'s worked example: fills survive under error text, but a
 *  fill never overrides the wedge or the error color).
 *
 *  #2874 item 8: solid borders no longer emit literal `border-{side}`
 *  declarations here — `parseCellStyleJson` already folded them into a
 *  single `--sh-c-border-shadow` inset box-shadow custom property
 *  (composed into `box-shadow` by SheetGrid.svelte's `.sh-c.is-anchor`/
 *  `.sh-c--ghost.is-typing` rules, alongside the accent/selection ring).
 *  A styled right/bottom side also needs its base `.sh-c` hairline
 *  suppressed inline (`border-right:none`/`border-bottom:none`) since,
 *  unlike the old literal-border rule, a box-shadow doesn't overwrite
 *  that property itself. Dashed borders have no box-shadow equivalent and
 *  stay on the literal `border-{side}` fields `parseCellStyleJson` still
 *  produces for that case — those are applied here unconditionally, same
 *  as before.
 *
 *  Returns `''` (no `style` attribute needed) when `styleJson` is
 *  null/undefined/empty/malformed, or when every parsed declaration was
 *  filtered out by the carve-out above (e.g. an error cell whose only
 *  styling is an ink color). */
export function cellInlineStyle(styleJson: string | null | undefined, kind: CellKind): string {
  const parsed = parseCellStyleJson(styleJson);
  const decls: string[] = [];
  if (parsed.background) decls.push(`background:${parsed.background}`);
  if (parsed.color && kind !== 'err') decls.push(`color:${parsed.color}`);
  if (parsed.boxShadow) decls.push(`--sh-c-border-shadow:${parsed.boxShadow}`);
  if (parsed.borderTop) decls.push(`border-top:${parsed.borderTop}`);
  if (parsed.borderRight) decls.push(`border-right:${parsed.borderRight}`);
  if (parsed.borderBottom) decls.push(`border-bottom:${parsed.borderBottom}`);
  if (parsed.borderLeft) decls.push(`border-left:${parsed.borderLeft}`);
  if (parsed.styledRight) decls.push('border-right:none');
  if (parsed.styledBottom) decls.push('border-bottom:none');
  return decls.join(';');
}

/** Full class list for a data cell — kind-based classes, type-based
 *  alignment class, and selection-state classes (anchor/range/edges). */
export function cellClass(
  row: number,
  col: number,
  cell: SheetCell | null,
  colType: string,
  selection: { anchor: CellCoord | null; range: CellRange | null },
  choices?: string[]
): string {
  const kind = cellKind(cell, colType, choices);
  const classes = ['sh-c'];
  // #2874 item 55: no `sh-c--calc` class — the formula-cell tint it fed
  // (`.sh-c--calc` / `.sh-c.is-range.sh-c--calc` in SheetGrid.svelte) was
  // removed per Matt's ruling ("no formula indicators anywhere"; users
  // add borders/backgrounds themselves if they want them). The class had
  // zero other consumers (grep-verified: no other CSS rule, no JS
  // selector, no test keyed off anything but its own presence) so it is
  // dropped outright rather than kept dead. `kind === 'calc'` itself is
  // UNCHANGED and still gates the type-alignment class below — that's a
  // separate, non-visual concern (numeric/date/etc. alignment for a
  // resolved formula value), not part of this removal.
  if (kind === 'err') classes.push('sh-c--err');
  else if (kind === 'raw') classes.push('sh-c--raw');
  else if (kind === 'mis') classes.push('sh-c--mis');

  if (kind === 'normal' || kind === 'calc') {
    const typeClass =
      colType === 'number' ? 'sh-c--num' :
      colType === 'currency' ? 'sh-c--currency' :
      colType === 'date' ? 'sh-c--date' :
      colType === 'checkbox' ? 'sh-c--check' : 'sh-c--text';
    classes.push(typeClass);
  }

  const coord: CellCoord = { row, col };
  if (isInRange(coord, selection.range)) {
    classes.push('is-range');
    const edges = rangeEdges(coord, selection.range);
    if (edges.top) classes.push('is-redge-t');
    if (edges.bottom) classes.push('is-redge-b');
    if (edges.left) classes.push('is-redge-l');
    if (edges.right) classes.push('is-redge-r');
  }
  if (isAnchor(coord, selection.anchor)) classes.push('is-anchor');
  return classes.join(' ');
}
