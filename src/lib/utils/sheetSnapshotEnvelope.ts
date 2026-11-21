// sheetSnapshotEnvelope.ts — parses the versioned sheet-snapshot JSON
// envelope (`note_versions.content` for `source IN ('sheet_snapshot',
// 'pre_restore')`, backend `sheets_snapshots.rs` §33.9.1) into a dense,
// render-ready preview grid for the sheet-native version-history panel
// (#2339 PR 15b-beta). Pure, DOM-free, fully unit-tested — mirrors
// `sheetGridPivot.ts`'s sparse-to-dense contract but operates on the
// envelope's OWN column/row/cell shape (a snapshot is frozen historical
// data, never a live `SheetColumn`/`SheetRow`/`SheetCell` fetched via
// `get_sheet_data`).
//
// Envelope shape, matching `sheets_snapshots.rs`'s `SheetSnapshotEnvelope`/
// `EnvelopeColumn`/`EnvelopeRow`/`EnvelopeCell` structs byte-for-byte
// (field names, snake_case, exactly what `serde_json` serializes):
//   { v: 1, columns: [{id,name,col_type,options_json,width,sort_order}],
//           rows: [{id,sort_order,height}],
//           cells: [{row_id,column_id,value,style_json}] }
//
// `value` is ALWAYS a raw stored value or an `=`-prefixed formula STRING
// — a snapshot never stores a computed value (ADR-0066 D4, restated at
// §33.9.1). The preview therefore shows raw formula text for a formula
// cell, never an illustrative computed number — a deliberate deviation
// from the design mockup's own sample data (which shows literal computed
// totals), documented on `pivotSnapshotEnvelope` below.

export interface EnvelopeColumn {
  id: string;
  name: string;
  col_type: string;
  options_json: string;
  width: number;
  sort_order: number;
}

export interface EnvelopeRow {
  id: string;
  sort_order: number;
  height: number;
}

export interface EnvelopeCell {
  row_id: string;
  column_id: string;
  value: string;
  style_json: string;
}

export interface SheetSnapshotEnvelope {
  v: number;
  columns: EnvelopeColumn[];
  rows: EnvelopeRow[];
  cells: EnvelopeCell[];
}

/** The only envelope shape this build understands — mirrors the backend's
 *  own `ENVELOPE_VERSION` constant (`sheets_snapshots.rs`). A restore
 *  against any other `v` fails cleanly on the BACKEND (this module never
 *  attempts a restore itself); this constant is used only so the preview
 *  can show the same "unrecognized version" fallback rather than
 *  mis-rendering a shape it doesn't understand. */
export const ENVELOPE_VERSION = 1;

/** Parses `note_versions.content` for a sheet snapshot row. Returns
 *  `null` (never throws) on invalid JSON, a missing/non-numeric `v`, an
 *  unrecognized `v`, or a shape missing any of the three required arrays
 *  — the preview pane renders its own "couldn't read this snapshot"
 *  fallback in that case rather than crashing. */
export function parseSheetSnapshotEnvelope(content: string): SheetSnapshotEnvelope | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== 'object') return null;
  const obj = parsed as Record<string, unknown>;
  if (typeof obj.v !== 'number' || obj.v !== ENVELOPE_VERSION) return null;
  if (!Array.isArray(obj.columns) || !Array.isArray(obj.rows) || !Array.isArray(obj.cells)) return null;
  return {
    v: obj.v,
    columns: obj.columns as EnvelopeColumn[],
    rows: obj.rows as EnvelopeRow[],
    cells: obj.cells as EnvelopeCell[],
  };
}

/** Fallback render width/height — matches `sheetGridPivot.ts`'s own
 *  `DEFAULT_COLUMN_WIDTH`/`DEFAULT_ROW_HEIGHT` fallback rule (a stored
 *  `0` is never a valid render dimension), applied identically to
 *  snapshot columns/rows since the envelope can carry the same
 *  freshly-created `0` values a live sheet can. */
const DEFAULT_COLUMN_WIDTH = 148;
const DEFAULT_ROW_HEIGHT = 32;

export interface PreviewDenseCell {
  col: number;
  row: number;
  cell: EnvelopeCell | null;
}

export interface PreviewDenseRow {
  row: EnvelopeRow;
  height: number;
  cells: PreviewDenseCell[];
}

export interface PivotedSnapshot {
  columns: Array<EnvelopeColumn & { effectiveWidth: number }>;
  rows: PreviewDenseRow[];
  /** Count of cells whose `value` is a `=`-prefixed formula — drives the
   *  preview header's "K computed" metric. */
  computedCount: number;
}

/** Pivots a parsed envelope's sparse `cells` list into the dense grid
 *  `ShSnapshotPreview.svelte` renders from — same
 *  sort-by-sort_order-then-id ordering rule as `sheetGridPivot.ts`'s
 *  `pivotSheetData`, for the same defensive reason (a hand-built
 *  fixture, or a future caching layer, might hand in out-of-order
 *  arrays even though the backend's own queries are already ordered). */
export function pivotSnapshotEnvelope(envelope: SheetSnapshotEnvelope): PivotedSnapshot {
  const sortedColumns = [...envelope.columns].sort(sortOrderThenId);
  const sortedRows = [...envelope.rows].sort(sortOrderThenId);

  const cellByRowThenCol = new Map<string, Map<string, EnvelopeCell>>();
  let computedCount = 0;
  for (const cell of envelope.cells) {
    if (cell.value.startsWith('=')) computedCount++;
    let byCol = cellByRowThenCol.get(cell.row_id);
    if (!byCol) {
      byCol = new Map();
      cellByRowThenCol.set(cell.row_id, byCol);
    }
    byCol.set(cell.column_id, cell);
  }

  const rows: PreviewDenseRow[] = sortedRows.map((row, r) => {
    const byCol = cellByRowThenCol.get(row.id);
    const cells: PreviewDenseCell[] = sortedColumns.map((col, c) => ({
      col: c,
      row: r,
      cell: byCol?.get(col.id) ?? null,
    }));
    return { row, height: row.height || DEFAULT_ROW_HEIGHT, cells };
  });

  const columns = sortedColumns.map((c) => ({ ...c, effectiveWidth: c.width || DEFAULT_COLUMN_WIDTH }));

  return { columns, rows, computedCount };
}

function sortOrderThenId(a: { sort_order: number; id: string }, b: { sort_order: number; id: string }): number {
  if (a.sort_order !== b.sort_order) return a.sort_order - b.sort_order;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/** Tolerant `style_json` → inline-style-prop parser, the READ path for
 *  the cell-styling feature (§33.6, design-phase — no fixed DB schema,
 *  `style_json` stays an opaque JSON blob). Originally written for the
 *  read-only snapshot preview only; #2339 PR 15d-beta's `ShStylePanel`/
 *  `buildCellStyleJson` (`sheetCellStyle.ts`) is now the WRITE path
 *  producing this same shape, including the beta-introduced multi-border
 *  array format (`bd: BorderSpec[]`, one entry per styled side) — this
 *  parser is updated to read BOTH that array shape and the original
 *  alpha-era single-object `bd: {side,w,c}` shape (backward compat: any
 *  style_json written by alpha before this PR still parses identically).
 *  Malformed JSON or an unrecognized shape returns `{}` (no styling),
 *  never throws, since a snapshot from ANY future style schema must
 *  still render its base value even if this parser can't interpret its
 *  styling yet. */
export interface ParsedCellStyle {
  background?: string;
  color?: string;
  /** Comma-joined `inset` box-shadow segments for every SOLID styled
   *  side, in deterministic top/bottom/left/right order regardless of
   *  the input `bd` array's order (#2874 item 8). Consumed by
   *  `cellInlineStyle` (sheetCellRender.ts), which sets it as the
   *  `--sh-c-border-shadow` inline custom property so SheetGrid.svelte's
   *  state-class rules (`.sh-c.is-anchor`, `.sh-c--ghost.is-typing`) can
   *  compose it into their own `box-shadow` list alongside the
   *  accent/selection ring, instead of a literal `border-{side}` rule
   *  colliding at the shared 1px edge with the neighboring cell's own
   *  border/hairline and rendering every weight too faint. */
  boxShadow?: string;
  /** True when a SOLID right-side border is present — signals
   *  `cellInlineStyle` to emit `border-right:none`, suppressing the base
   *  `.sh-c` hairline on that edge so the box-shadow segment (which does
   *  NOT replace the literal border like the old `border-right` rule
   *  did) isn't fighting a visible hairline underneath it. */
  styledRight?: boolean;
  /** Same as `styledRight`, for the bottom edge. */
  styledBottom?: boolean;
  /** Dashed-border fallback ONLY — `box-shadow` has no dashed equivalent,
   *  so a `BorderSpec` entry with `dashed: true` stays on the original
   *  literal `border-{side}` CSS path instead of being folded into
   *  `boxShadow` above. Known limitation: a dashed border still exhibits
   *  the adjacent-cell collapse this whole fix exists to solve for solid
   *  borders (#2874 item 8, recorded for future work). */
  borderTop?: string;
  borderRight?: string;
  borderBottom?: string;
  borderLeft?: string;
}

export function parseCellStyleJson(styleJson: string | null | undefined): ParsedCellStyle {
  if (!styleJson) return {};
  let obj: unknown;
  try {
    obj = JSON.parse(styleJson);
  } catch {
    return {};
  }
  if (!obj || typeof obj !== 'object') return {};
  const raw = obj as Record<string, unknown>;
  const out: ParsedCellStyle = {};
  if (typeof raw.bg === 'string') out.background = raw.bg;
  if (typeof raw.fg === 'string') out.color = raw.fg;
  const bd = raw.bd;

  // Solid sides are collected here (keyed by side, not appended in input
  // order) so the final `boxShadow` join can always emit them in the
  // deterministic top/bottom/left/right order regardless of what order
  // the `bd` array (or, alpha-era, single object) listed them in.
  const shadowSegments: Partial<Record<'top' | 'bottom' | 'left' | 'right', string>> = {};

  const applyDashedBorderEntry = (side: string, w: number, c: string) => {
    const rule = `${w}px dashed ${c}`;
    if (side === 'top') out.borderTop = rule;
    else if (side === 'right') out.borderRight = rule;
    else if (side === 'bottom') out.borderBottom = rule;
    else if (side === 'left') out.borderLeft = rule;
  };

  const applyBorderEntry = (entry: unknown) => {
    if (!entry || typeof entry !== 'object') return;
    const { side, w, dashed, c: cRaw } = entry as Record<string, unknown>;
    if (typeof side !== 'string' || typeof w !== 'number' || typeof cRaw !== 'string') return;
    let c: string = cRaw;
    if (c === 'var(--border-strong)') c = 'var(--sheet-border-styled)';
    if (dashed === true) {
      // Dashed can't be represented as box-shadow — stays on the literal
      // border-* path (see ParsedCellStyle's borderTop/etc. doc comment).
      applyDashedBorderEntry(side, w, c);
      return;
    }
    if (side === 'top') shadowSegments.top = `inset 0 ${w}px 0 0 ${c}`;
    else if (side === 'bottom') {
      shadowSegments.bottom = `inset 0 -${w}px 0 0 ${c}`;
      out.styledBottom = true;
    } else if (side === 'left') shadowSegments.left = `inset ${w}px 0 0 0 ${c}`;
    else if (side === 'right') {
      shadowSegments.right = `inset -${w}px 0 0 0 ${c}`;
      out.styledRight = true;
    }
  };

  // Beta's multi-border format: `bd` is an ARRAY of border specs, one per
  // styled side (`buildCellStyleJson` in `sheetCellStyle.ts` is the only
  // writer of this shape). Backward compat: alpha-era `style_json` values
  // stored `bd` as a single object (one side only) — still handled below.
  if (Array.isArray(bd)) {
    for (const entry of bd) applyBorderEntry(entry);
  } else if (bd && typeof bd === 'object') {
    applyBorderEntry(bd);
  }

  const joined = (['top', 'bottom', 'left', 'right'] as const)
    .map((side) => shadowSegments[side])
    .filter((seg): seg is string => !!seg)
    .join(', ');
  if (joined) out.boxShadow = joined;

  return out;
}
