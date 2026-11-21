// sheetSrcLabel.ts — pure mapping from a `note_versions.source` tag to the
// sheet-native version-history label the design calls `SrcLabel` (#2339
// PR 15b-beta, `docs/design/sheets/sheets-surfaces.jsx:129-160`,
// `sheets.css:294-299`).
//
// The design specifies FOUR source variants — sheet / agent / restore /
// manual — each with its own glyph, CSS class, and label word. The real
// backend (`sheets_snapshots.rs`) currently only ever writes TWO distinct
// source strings for a sheet note's `note_versions` rows:
//   - `"sheet_snapshot"` — every grid-mutation capture (human OR AI edit;
//     `sheets_snapshots.rs`'s capture chokepoint is shared by both, see
//     `update_sheet_cell_internal` / the AI tool's `set_sheet_cells` path
//     — there is currently no distinct "this snapshot was an agent edit"
//     source tag anywhere in the backend).
//   - `"pre_restore"` — the guaranteed pre-restore capture inside
//     `restore_sheet_snapshot_internal` (reused verbatim from the
//     general-note restore path, Matt's ruling 2026-09-04 (2)).
//
// This module maps ALL FOUR design variants so the mapping function
// itself is complete/testable and forward-compatible with a future
// backend change that distinguishes agent-authored snapshots (an
// `agent`/`manual` tag is not invented or written by this PR — see
// `sheets_snapshots.rs`, unchanged) — but only `sheet`/`restore` are
// actually reachable with real sheet version-history data today.
// `mapSheetVersionSource` never throws on an unrecognized tag; it falls
// back to the neutral `manual` treatment so a future/unknown source
// string still renders something sane instead of crashing the panel.

export type SheetSrcKind = 'sheet' | 'agent' | 'restore' | 'manual';

export interface SheetSrcLabelInfo {
  kind: SheetSrcKind;
  /** CSS class suffix appended to `.sh-src` (e.g. `sh-src--sheet`) — the
   *  `manual` variant intentionally has NO modifier class (design: "Neutral
   *  surface-2, unchanged from today"), matching `sheets.css`'s own rule
   *  set (only `.sh-src--sheet`/`--agent`/`--restore` exist; `.sh-src`'s
   *  base rule alone IS the manual treatment). */
  cssClass: string;
  /** Single-character/short glyph rendered in the chip (design uses real
   *  SVG icons; this text-only build uses the same glyph convention
   *  `sheetCellRender.ts`'s `typeGlyph` already established for Sheets —
   *  no new icon dependency). */
  glyph: string;
  /** The label word — "Sheet edit" / "Agent" / "Before restore" /
   *  "Manual edit", verbatim from the design's `SrcLabel` map. */
  label: string;
}

const SRC_LABELS: Record<SheetSrcKind, Omit<SheetSrcLabelInfo, 'kind'>> = {
  sheet: { cssClass: 'sh-src--sheet', glyph: '▦', label: 'Sheet edit' },
  agent: { cssClass: 'sh-src--agent', glyph: '◎', label: 'Agent' },
  restore: { cssClass: 'sh-src--restore', glyph: '↻', label: 'Before restore' },
  manual: { cssClass: '', glyph: '✎', label: 'Manual edit' },
};

/** Maps a raw `note_versions.source` string (as stored for a SHEET note)
 *  to its `SheetSrcKind` + display info. `"sheet_snapshot"` → `sheet`,
 *  `"pre_restore"` → `restore`; `"ai_edit"`/`"agent_edit"`-shaped values
 *  map to `agent` for forward-compatibility (unreachable with the current
 *  backend, which never writes these for a sheet note — see module doc
 *  comment); anything else (including today's unreachable-in-practice
 *  `"manual_edit"`/unknown values) falls back to `manual`. */
export function mapSheetVersionSource(source: string): SheetSrcLabelInfo {
  const kind = classifySource(source);
  return { kind, ...SRC_LABELS[kind] };
}

function classifySource(source: string): SheetSrcKind {
  if (source === 'sheet_snapshot') return 'sheet';
  if (source === 'pre_restore') return 'restore';
  if (source === 'agent_edit' || source === 'ai_edit' || source === 'agent') return 'agent';
  return 'manual';
}

/** The secondary detail text shown after the label (design: "· 12 cells" /
 *  "· structure" / an agent's name / nothing for restore & manual). This
 *  PR has no per-snapshot diff/change-count data source (`note_versions`
 *  stores no cell-count or diff field — computing one would require
 *  fetching and diffing every version's full envelope, out of scope for
 *  this PR's read-only history panel), so this always returns `null` for
 *  every kind; the caller renders no `· detail` suffix when `null`. Kept
 *  as its own pure function (rather than inlined in the Svelte template)
 *  so the "we deliberately don't compute this yet" decision has one named
 *  , testable, documented seam. */
export function sheetSrcLabelDetail(_kind: SheetSrcKind): string | null {
  return null;
}
