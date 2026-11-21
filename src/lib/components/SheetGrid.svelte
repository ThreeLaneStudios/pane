<script lang="ts">
  // SheetGrid.svelte — grid renderer for the Sheets note type (#2339 PR 9
  // read-only grid, PR 10 typed-column editing). Owns: data loading
  // (`get_sheet_data`), the sparse->dense pivot, selection/keyboard-nav
  // state, cell/header editing lifecycle, ghost typing-to-commit, and CSS
  // Grid rendering with sticky header/gutter/corner.
  //
  // PR 10 scope (Matt's 8 rulings + carry-forward fixes, see
  // ~/Shared/ExampleApp/docs/audits/2026-09-04-sheets-pr10-typed-editing-scope.md):
  //   - Six per-type cell editors (sheetEditors/*.svelte), opened by
  //     double-click (edit-in-place, pre-filled) or by typing a printable
  //     character while the cell is the sole anchor (type-to-overwrite,
  //     clears the cell — the PRIMARY entry path, ruling 1). Enter is
  //     navigation only — it never opens an editor.
  //   - Checkbox has no editor: click or Space toggles immediately.
  //   - Ghost typing-to-commit: a printable keystroke on a ghost coordinate
  //     creates the row/column (+ header-rename for new columns) then
  //     writes the typed character, per the four cases in scope §4.
  //   - Two selection-reducer bugfixes (Tab row-wrap, Enter-in-range) live
  //     in sheetSelection.ts, consumed unchanged here.
  //   - `.sh-c--raw` fallback + `cellKind`/`displayValue`/`cellClass` now
  //     live in sheetCellRender.ts (testable, carries the PR10 raw-vs-calc
  //     fix). Per-type value parsing lives in sheetCellCommit.ts.
  //
  // PR 11 scope (Matt's 7 rulings, see
  // ~/Shared/ExampleApp/docs/audits/2026-09-04-sheets-pr11-header-menu-scope.md):
  //   - `ShHeaderMenu.svelte` — a 5-verb popover opened by clicking anywhere
  //     on a real column's header cell (which ALSO still whole-column-
  //     selects, unchanged from PR 9/10). Rename hands off to PR 10's
  //     EXISTING inline `.sh-h__ren` input (ruling 1) — this component owns
  //     that mechanism, the menu just calls `startHeaderRename` on close.
  //   - Type-change/delete are structural — both call their backend command
  //     then `refetchSheetData()`, matching every other structural mutation
  //     in this file. Delete additionally clamps selection to the nearest
  //     remaining real column (ruling 4) via `clampColumnAfterDelete`.
  //   - Sort/filter (`sheetView.ts`'s `applySort`/`applyFilter`) are PURE
  //     VIEW transforms over the already-pivoted grid — no backend call,
  //     `viewState` lives entirely in this component's own state, single-
  //     column only (ruling 5: `sortColumnId: string | null`, replaced not
  //     appended on every new sort click).
  //   - ShMismatch (ruling 7) rides `cellKind`'s existing `'mis'` branch
  //     (sheetCellRender.ts, wired to `isTypeMismatch` in sheetView.ts) —
  //     rendered here as the `.sh-w--mis` corner wedge plus a hover/select-
  //     triggered `.sh-pop` tooltip, same shell class the header menu uses.
  //
  // Editing state (`editing`, `headerRename`, `ghostTyping`) is intentionally
  // NOT part of the sheetSelection.ts reducer — that module stays
  // selection/navigation-only (its own documented boundary, preserved from
  // PR 9). Editing state is transient per-keystroke; selection state
  // persists across commits — mixing them would undo PR 9's testability
  // win for the reducer.
  import { onMount, onDestroy } from 'svelte';
  import {
    ENTITY_SHEET,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';
  import * as api from '$lib/api';
  import type { SheetColumn, SheetRow, SheetCell, SheetData } from '$lib/types';
  import { pivotSheetData, DEFAULT_COLUMN_WIDTH, GHOST_COLUMN_WIDTH, DEFAULT_ROW_HEIGHT, type PivotedSheet } from '$lib/utils/sheetGridPivot';
  import { computeNewSize, formatReadout, MIN_COLUMN_WIDTH, MIN_ROW_HEIGHT } from '$lib/utils/sheetResize';
  import {
    reduceSheetSelection,
    INITIAL_SHEET_SELECTION,
    isInRange,
    isAnchor,
    clampCoord,
    type SheetSelectionState,
    type SheetBounds,
    type CellCoord,
  } from '$lib/utils/sheetSelection';
  import { cellKind, displayValue, cellClass, cellInlineStyle, typeGlyph, currencyCode, formatCurrencyValue } from '$lib/utils/sheetCellRender';
  import { valueForCommit, toggleCheckboxValue, isoToDateMask } from '$lib/utils/sheetCellCommit';
  import { applySort, applyFilter, clearSort, clearFilter, selectChoicesFromOptionsJson, filterSupported, type SheetViewState } from '$lib/utils/sheetView';
  import { extractFormulaRefRanges, isCellInAnyRefRange, refRangeEdges, type FormulaRefRange } from '$lib/utils/sheetFormula';
  import { parseCellStyleRaw, buildCellStyleJson, mergeStylePatch, expandSelection, detectUnanimity, detectBorderUnanimity, resolvedBorderSides, type CellStyleInput, type BorderSpec } from '$lib/utils/sheetCellStyle';
  import SheetCellEditorText from '$lib/components/sheetEditors/SheetCellEditorText.svelte';
  import SheetCellEditorDate from '$lib/components/sheetEditors/SheetCellEditorDate.svelte';
  import SheetCellEditorSelect from '$lib/components/sheetEditors/SheetCellEditorSelect.svelte';
  import SheetCellEditorFormula from '$lib/components/sheetEditors/SheetCellEditorFormula.svelte';
  import ShHeaderMenu from '$lib/components/ShHeaderMenu.svelte';
  import ShErrorPopover from '$lib/components/ShErrorPopover.svelte';
  import SheetViewStrip from '$lib/components/SheetViewStrip.svelte';
  import ShCellContextMenu from '$lib/components/ShCellContextMenu.svelte';
  import ShGhostCellContextMenu from '$lib/components/ShGhostCellContextMenu.svelte';
  import ShStylePanel from '$lib/components/ShStylePanel.svelte';

  /** Shape pushed to `onSelectionInfo` (#2339 PR 14) — everything
   *  `FormulaBar.svelte` (mounted in Editor.svelte, NOT inside this
   *  component) needs to render the anchor cell's address/value/editing/
   *  error state, recomputed reactively any time selection/editing/error
   *  changes (see the `formulaBarInfo` $derived.by below). */
  export interface FormulaBarInfo {
    cref: string;
    value: string;
    editing: boolean;
    error: string | null;
    /** False for checkbox/select anchors — those columns have no typed-
     *  buffer editor at all (checkbox: click-toggle only; select: dropdown
     *  only), so the bar's own input has nothing sensible to drive. */
    editable: boolean;
    /** #2339 PR 14 fix (bar/cell caret parity): the shared editing
     *  session's live caret position — mirrors `EditingCell.selectionStart`/
     *  `selectionEnd` below. `null` when nothing is being edited, or when
     *  no selection has been reported yet for this session (the bar/cell
     *  editor components both treat `null` as "fall back to end-of-
     *  text"). */
    selectionStart: number | null;
    selectionEnd: number | null;
    /** #2874 item 16 Part C: the column the toolbar's Sort/Filter/Formula
     *  buttons would act on — `colSel ?? anchor.col`, the SAME
     *  forgiving-source resolution `toolbarSort`/`toolbarFilter` use
     *  internally, exposed here so `Editor.svelte` can derive
     *  `sortEnabled`/`filterEnabled` without a second selection feed.
     *  `null` when nothing resolves (nothing selected, or the resolved
     *  index is a ghost column past `pivoted.columns.length`). */
    selectedColumnIndex: number | null;
    /** The resolved column's `col_type`, or `null` alongside
     *  `selectedColumnIndex === null`. `Editor.svelte` feeds this straight
     *  into `filterSupported()` to compute `filterEnabled`. */
    selectedColumnType: string | null;
  }

  interface Props {
    noteId: string;
    /** Called once the sheet finishes its first successful load, with the
     *  raw SheetData — used by SheetStatusBar (cols/rows counts) and
     *  SheetToolbar in the parent (Editor.svelte) without a second fetch. */
    onLoaded?: (data: SheetData) => void;
    /** #2339 PR 14: fired reactively (via `$effect`) whenever the anchor
     *  cell's address/value/editing/error state changes — the single feed
     *  `FormulaBar.svelte` (mounted in Editor.svelte) renders from. */
    onSelectionInfo?: (info: FormulaBarInfo) => void;
    /** #2874 item 11: previously threaded through to `ShCellContextMenu`'s
     *  now-removed "Export sheet as CSV…" row. That row is gone —
     *  `ShSheetMenu.svelte` keeps its own independent Export CSV row
     *  (wired via `SheetToolbar.svelte`'s own `onExportCsv` prop, not this
     *  one), zero shared code path. This prop is retained on the Props
     *  interface for caller compatibility (`Editor.svelte` still passes it)
     *  but is currently unused inside this component. */
    onExportCsv?: () => void;
  }

  let { noteId, onLoaded, onSelectionInfo, onExportCsv }: Props = $props();

  const GHOST_COLS = 3;
  const GHOST_ROWS = 4;
  const HEAD_H = 38;
  const GUTTER_W = 40;

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let sheetData: SheetData | null = $state(null);
  let pivoted: PivotedSheet = $state({ columns: [], rows: [] });
  let selection: SheetSelectionState = $state(INITIAL_SHEET_SELECTION);
  let gridEl: HTMLDivElement | null = $state(null);

  // Drag-to-select state (mousedown on a cell, drag to extend range).
  // CSS :hover handles the ghost hover-tint visual (ruling 4) — no JS
  // hover-tracking state is needed for that.
  let isDragSelecting = false;

  // ── Editing lifecycle state (#2339 PR 10, §3) ───────────────────────────

  interface EditingCell {
    row: number;
    col: number;
    colType: string;
    /** Live editor buffer. For `date` this is the MM/DD/YYYY mask, not
     *  ISO — conversion happens at commit time via `valueForCommit`. For
     *  `select` this mirrors the cell's current raw_value (used only to
     *  mark the current choice `is-on`; commit is per-choice-click, not
     *  buffer-based). */
    value: string;
    /** #2339 PR 14: whether THIS edit session opened in formula-editing
     *  mode — decided ONCE, from the value the session opened WITH (see
     *  `openEditorForCell`), never re-evaluated per keystroke. This is
     *  deliberate: re-deriving it reactively from the live buffer on every
     *  keystroke (`editing.value.startsWith('=')`) would swap the
     *  rendered editor COMPONENT out from under an in-progress edit the
     *  instant a `=` appears anywhere in the buffer mid-type, tearing
     *  down and recreating the underlying `<input>` DOM node and losing
     *  any live reference to it (breaks e.g. a caller that grabbed the
     *  input element before typing the rest of a value). Freezing the
     *  decision at open time still satisfies "typing `=` as the FIRST
     *  character opens formula mode" — that's exactly the type-to-
     *  overwrite primary entry path (ruling 1), where `openEditorForCell`
     *  is called with `clearWithChar: '='` and the session is born
     *  already in formula mode, before any component ever mounts. */
    isFormula: boolean;
    /** #2339 PR 14 fix (bar/cell caret parity): the live caret/selection
     *  position within `value`, shared between whichever surface (bar or
     *  in-cell editor) last had focus — both `FormulaBar.svelte`'s and
     *  `SheetCellEditorFormula.svelte`'s own `onSelectionChange` callbacks
     *  write here (via `barSetSelection` below), and both read it back
     *  (via `formulaBarInfo` / the template's `selectionStart`/
     *  `selectionEnd` props) so a focus handoff mid-edit lands the caret
     *  where the user left it instead of jumping to end-of-text. `null`
     *  until the first selection change is reported for this session. */
    selectionStart: number | null;
    selectionEnd: number | null;
    /** #2874 item 12 Part D: which surface STARTED this editing session —
     *  'cell' for every existing entry point (double-click, type-to-edit,
     *  ghost-row typing) and 'bar' only for barBeginEdit (a click on the
     *  formula bar's idle display/placeholder). Consumed by the three
     *  in-cell editor components' own mount-focus $effects (Text/Date/
     *  Formula) so they can skip stealing focus away from the bar's own
     *  input when the bar itself opened the session — the bar's focus()
     *  effect (FormulaBar.svelte, Part B) is deliberately unguarded on ITS
     *  side of this exact race, so exactly one of the two surfaces needs
     *  to defer, and this flag says which one. Defaults to 'cell' at every
     *  call site EXCEPT barBeginEdit — see openEditorForCell. */
    origin: 'cell' | 'bar';
  }

  let editing: EditingCell | null = $state(null);
  /** Set when the most recent commit attempt for `editing`'s cell was
   *  rejected by the backend (formula parse failure, or a network error) —
   *  rendered as inline error text under the editor. Cleared whenever a
   *  new editor opens or a commit succeeds. */
  let editorError = $state<string | null>(null);

  /** Inline header-rename state (#2339 PR 10, §4) — appears only as a
   *  direct consequence of a ghost-column commit, never from clicking an
   *  existing column's header (that's still SELECT_COLUMN, unchanged). */
  interface HeaderRename {
    col: number;
    value: string;
  }
  let headerRename: HeaderRename | null = $state(null);

  /** Marks a single ghost coordinate mid-flight through ghost-commit (§4) —
   *  drives the `.sh-c--ghost.is-typing` visual (full opacity + cyan ring,
   *  typed character shown) while the create+update round-trip is in
   *  flight, and is cleared (reverting to plain ghost rendering) on either
   *  success (replaced by the real cell post-refetch) or failure. */
  interface GhostTyping {
    row: number;
    col: number;
    char: string;
  }
  let ghostTyping: GhostTyping | null = $state(null);

  // ── Column header menu (#2339 PR 11, ShHeaderMenu) ─────────────────────

  /** The REAL column index whose `ShHeaderMenu` popover is open, or `null`.
   *  §33.3.6 (PR #2924, commit e7e341ce) split the interaction: left-click
   *  on a header selects the whole column and opens nothing
   *  (`handleColumnHeaderClick`); right-click opens this popover
   *  (`handleColumnHeaderContextMenu`), anchored to the clicked header via
   *  `headerMenuAnchor`. Supersedes the Sep 4 "one click, both effects"
   *  ruling. Ghost column headers have no menu (no column to act on yet). */
  let openHeaderMenu: number | null = $state(null);
  let headerMenuAnchor: HTMLElement | null = $state(null);

  // ── Sort/filter view-state (#2339 PR 11, sheetView.ts) ───────────────

  /** Pure view-state (never persisted, never sent to the backend) — which
   *  column is sorted/filtered and how. Applied to `pivoted.rows` at
   *  render time via `visibleRows` below; the underlying `pivoted`/
   *  `sheetData` never reflect sort/filter (§33.7: "view only"). */
  let viewState: SheetViewState = $state({
    sortColumnId: null,
    sortDirection: null,
    filterColumnId: null,
    filterValues: null,
  });

  /** A permutation of indexes INTO `pivoted.rows`, in DISPLAY order — i.e.
   *  `rowOrder[displayRowIndex] = underlyingPivotedRowsIndex`. This is the
   *  single seam sort/filter cross into every row-addressing codepath in
   *  this file: selection/keyboard-nav/editing/ghost-commit all address
   *  rows by DISPLAY index (matching what's actually rendered, and what
   *  the user clicked/arrowed to) — every lookup of the underlying
   *  `SheetRow`/cells for a given display row goes through
   *  `pivoted.rows[rowOrder[displayRow]]`, via the `underlyingRow` helper
   *  below, rather than indexing `pivoted.rows` directly.
   *
   *  With no active sort/filter (`viewState` at its initial all-null
   *  state, true for every PR 9/10 codepath and every existing test),
   *  `applySort`/`applyFilter` are no-ops that return a same-order copy,
   *  so `rowOrder` is the IDENTITY permutation (`[0, 1, 2, ...]`) — every
   *  existing PR 9/10 behavior (bounds, ghost-row math, editing lookups)
   *  is byte-for-byte unchanged when no view is active. */
  let rowOrder: number[] = $derived.by(() => {
    if (pivoted.rows.length === 0) return [];
    const cells = sheetData?.cells ?? [];
    const bareRows = pivoted.rows.map((dr) => dr.row);
    const sorted = applySort(bareRows, cells, pivoted.columns, viewState.sortColumnId, viewState.sortDirection);
    const filtered = applyFilter(sorted, cells, pivoted.columns, viewState.filterColumnId, viewState.filterValues);
    const underlyingIndexById = new Map(pivoted.rows.map((dr, i) => [dr.row.id, i]));
    const order: number[] = [];
    for (const r of filtered) {
      const idx = underlyingIndexById.get(r.id);
      if (idx !== undefined) order.push(idx);
    }
    return order;
  });

  /** The dense row at DISPLAY index `displayRow`, or `undefined` if
   *  `displayRow` is out of the visible (post-sort/filter) real-row range
   *  (i.e. it's a ghost row, or a stale index from before a filter
   *  shrank the visible set). */
  function underlyingRow(displayRow: number) {
    const underlyingIdx = rowOrder[displayRow];
    return underlyingIdx === undefined ? undefined : pivoted.rows[underlyingIdx];
  }

  /** The DISPLAY index a given underlying `pivoted.rows` index currently
   *  renders at, or `-1` if that row is filtered out of view entirely.
   *  Used after a refetch (ghost-commit, type-change, delete) to convert
   *  a freshly-looked-up underlying row index back into the coordinate
   *  space selection/editing addresses. */
  function displayRowIndexFor(underlyingIdx: number): number {
    return rowOrder.indexOf(underlyingIdx);
  }

  /** `pivoted.rows`, permuted into DISPLAY order per `rowOrder` — the
   *  array the template's real-row `{#each}` actually iterates, in place
   *  of `pivoted.rows` directly. Row index `ri` in that `{#each}` is the
   *  DISPLAY row index, consistent with every handler above (which all
   *  address rows by display index via `underlyingRow`/`rowOrder`). */
  let visibleRows = $derived(rowOrder.map((idx) => pivoted.rows[idx]));

  // ── View strip (#2339 PR 12, SheetViewStrip) ─────────────────────────

  /** Resolved column objects for the strip's props (scope report §2/§3
   *  — pass a RESOLVED `SheetColumn`, not just an id, letting
   *  `SheetViewStrip` render the column's `name` without doing its own
   *  lookup). `null` when no sort/filter is active, or when the
   *  sorted/filtered column has since been deleted (`.find` returns
   *  `undefined` → coerced to `null`) — the strip's own presence-gate
   *  then correctly hides that chip. */
  let sortColumn = $derived(pivoted.columns.find((c) => c.id === viewState.sortColumnId) ?? null);
  let filterColumn = $derived(pivoted.columns.find((c) => c.id === viewState.filterColumnId) ?? null);

  /** Wired straight to `sheetView.ts`'s `clearSort`/`clearFilter` — PR 11
   *  exported both, uncalled until now; the strip's chip × buttons are
   *  their first real callers. Clearing one leaves the other's state
   *  (and its own chip/header-glyph/`rowOrder` contribution) untouched,
   *  since both functions spread-preserve the rest of `viewState`. */
  function handleClearSort() {
    viewState = clearSort(viewState);
  }

  function handleClearFilter() {
    viewState = clearFilter(viewState);
  }

  // ── Mismatch tooltip (#2339 PR 11, ShMismatch, ruling 7) ─────────────

  /** The (row, col) of the mismatched cell whose explanation tooltip is
   *  showing — set on hover/select of a `'mis'`-kind cell, cleared on
   *  mouseleave/deselect. Only one tooltip at a time. */
  let mismatchTooltip: CellCoord | null = $state(null);

  // ── Cell context menu (#2339 PR 15b-alpha (alpha), ShCellContextMenu) ─

  /** The right-clicked cell's (row, col), or `null` when the context menu
   *  is closed. Set by `handleCellContextMenu` below. Note this is
   *  independent of `selection` — right-clicking a cell that's already
   *  part of the active selection leaves the selection untouched (the
   *  menu's actions operate over the CURRENT selection, which may be a
   *  multi-cell range the right-clicked cell is merely one member of);
   *  right-clicking OUTSIDE the current selection re-selects to just that
   *  one cell first (see `handleCellContextMenu`). */
  let contextMenuCell: CellCoord | null = $state(null);
  /** Viewport coordinate (the triggering `MouseEvent`'s `clientX`/
   *  `clientY`) the popover anchors off — see `ShCellContextMenu.svelte`'s
   *  own doc comment for why this differs from every other Sheets
   *  popover's static-button-ref anchoring. */
  let contextMenuPos: { x: number; y: number } | null = $state(null);
  /** #2874 item 22: true when `contextMenuCell` refers to a ghost row and/
   *  or ghost column — drives which context-menu component renders (the
   *  reduced `ShGhostCellContextMenu` vs. the full `ShCellContextMenu`).
   *  Same ghost-detection idiom used elsewhere in this file (comparing
   *  against `rowOrder.length`/`pivoted.columns.length`). */
  let contextMenuIsGhost = $derived.by(() => {
    if (!contextMenuCell) return false;
    return contextMenuCell.row >= rowOrder.length || contextMenuCell.col >= pivoted.columns.length;
  });

  // ── Cell styling (#2339 PR 15d-beta, ShStylePanel) ──────────────────
  //
  // "Two doors, one panel": opened either from SheetToolbar's fill/text/
  // border buttons (via Editor.svelte calling `openStylePanel` through the
  // `bind:this` handle — same imperative-export pattern as `insertRow`/
  // `insertColumn`) or from ShCellContextMenu's "Style cells…" row (which
  // calls `openStylePanel` directly, no prop needed, same as that menu's
  // `onInsertRow`/`onInsertColumn`). Both entry points mount the SAME
  // `ShStylePanel` instance below, driven by the SAME unanimity-derived
  // props — there is no per-entry-point state.

  let stylePanelOpen = $state(false);
  /** Fixed viewport position for the panel — pinned just under the
   *  toolbar's style buttons (the only current entry points that need a
   *  position; the context-menu entry point re-anchors near its own
   *  click point via `contextMenuPos` at open time, captured into this
   *  same pair of fields so `ShStylePanel` never needs to know which
   *  door it was opened through). */
  let stylePanelPos = $state({ x: 0, y: 0 });
  /** #2874 item 16 Part B: the toolbar button element the panel was
   *  opened from, when opened through that door — threaded straight into
   *  `ShStylePanel`'s `anchorEl` prop so its `handleWindowClick` guard can
   *  exclude clicks that land back on the SAME button (the same-click
   *  self-close bug, `docs/audits/2026-09-08-item16-partB-focus-loss-
   *  diagnosis.md`). `null` for the context-menu door, which has no
   *  persistent button element (`ShStylePanel` simply skips the guard). */
  let stylePanelAnchorEl: HTMLElement | null = $state(null);

  /** Toolbar-facing imperative entry point (`Editor.svelte`'s
   *  `sheetGridRef.openStylePanel()`, called from `SheetToolbar`'s
   *  unified "Style cells" `onStyleClick` callback — #2874 item 20;
   *  previously three separate fill/text/border callbacks) AND the
   *  context menu's "Style cells…" row's direct call.
   *  Closes the context menu if it happens to be open (right-click →
   *  "Style cells…" is itself one of the two doors) so the two popovers
   *  never show at once.
   *
   *  #2874 item 16 Part B: now accepts the REAL toolbar button element
   *  (rather than only a raw `{x,y}` point) and positions the panel just
   *  under it via `getBoundingClientRect()` — `rect.bottom + 4` for the
   *  vertical offset, with a right-edge overflow flip (the panel is
   *  268px wide, `.sh-sty`'s fixed width in `ShStylePanel.svelte`) so it
   *  never renders off the right edge of the window. The context-menu
   *  door still passes a raw `{x,y}` point (it has no persistent button
   *  element to anchor to); the hardcoded `{12, 78}` literal survives
   *  ONLY as the true last-resort fallback — reachable if this is ever
   *  called with neither an element nor a point, which no real call site
   *  does today.
   *
   *  #2874 item 52 (Matt's ruling): when the panel would run past the
   *  bottom of the window, it flips up and opens above the cell instead
   *  of clipping. Ported from `ShCellContextMenu.svelte`'s own
   *  `flipUp`/`posTop`/`MENU_HEIGHT` triplet — the one existing
   *  vertical-flip pattern in this codebase (see that component's
   *  `MENU_HEIGHT` comment: "measured against the real rendered
   *  component during design review", same posture this `PANEL_HEIGHT`
   *  follows). `PANEL_HEIGHT` was measured the same way: the real
   *  rendered `.sh-sty` panel (all three sections — fill swatches, ink
   *  swatches, border sides/weight/dashed — plus the "Clear styling"
   *  button; none of these are conditionally rendered, so the height is
   *  constant regardless of prop state) is 304.875px tall at its fixed
   *  268px width (`PANEL_WIDTH`, confirmed to match via the same
   *  measurement) — rounded up to 305 so the clamp always has at least
   *  as much headroom as the real panel needs. Covers BOTH doors this
   *  function opens through: the anchor-element branch (opened from a
   *  cell) and the raw-point branch (opened from the right-click
   *  context menu's "Style cells…" row) — both had the same unconditional
   *  `y`, never clamped against `window.innerHeight`, before this fix.
   *  The existing horizontal clamp (`x + PANEL_WIDTH > window.innerWidth`)
   *  is unchanged. */
  export function openStylePanel(anchor?: HTMLElement | { x: number; y: number }) {
    const PANEL_WIDTH = 268;
    const PANEL_HEIGHT = 305;
    const viewportHeight = typeof window !== 'undefined' && (window as any).innerHeight ? (window as any).innerHeight : Infinity;
    if (anchor instanceof HTMLElement) {
      const rect = anchor.getBoundingClientRect();
      let x = rect.left;
      if (x + PANEL_WIDTH > window.innerWidth) {
        x = Math.max(0, window.innerWidth - PANEL_WIDTH - 4);
      }
      const flipUp = rect.bottom + 4 + PANEL_HEIGHT > viewportHeight;
      // #2874 item 57 (Matt's ruling): floor the flipped position at 0 so
      // a mid-screen flip-up on a short window can never clip off the TOP
      // of the window — same shape as the horizontal clamp above
      // (`Math.max(0, …)` wrapping the overflow expression, applied only
      // on the overflow branch). Only the flipped branch gets the floor;
      // the non-flipped `rect.bottom + 4` position is already correct.
      const y = flipUp ? Math.max(0, rect.top - 4 - PANEL_HEIGHT) : rect.bottom + 4;
      stylePanelPos = { x, y };
      stylePanelAnchorEl = anchor;
    } else {
      const point = anchor ?? { x: 12, y: 78 };
      const flipUp = point.y + PANEL_HEIGHT > viewportHeight;
      // #2874 item 57: same floor as the anchor branch above — only the
      // flipped position is clamped, the non-flipped `point.y` is untouched.
      stylePanelPos = { x: point.x, y: flipUp ? Math.max(0, point.y - PANEL_HEIGHT) : point.y };
      stylePanelAnchorEl = null;
    }
    stylePanelOpen = true;
    contextMenuCell = null;
    contextMenuPos = null;
  }

  function closeStylePanel() {
    stylePanelOpen = false;
    stylePanelAnchorEl = null;
  }

  /** Raw `style_json` strings for every cell the ACTIVE selection covers
   *  (via `expandSelection` — same anchor-or-range convention as
   *  `selectedCellCoords` above, but expressed through `sheetCellStyle.
   *  ts`'s own shared helper since the style panel's unanimity derivation
   *  and `applyStyleToSelection` both need the identical cell set). Ghost
   *  coordinates are skipped (no underlying cell to read/style). */
  function selectedStyleJsons(): Array<{ style_json: string }> {
    const coords = expandSelection({ anchor: selection.anchor, range: selection.range });
    const out: Array<{ style_json: string }> = [];
    for (const { row, col } of coords) {
      if (row >= rowOrder.length || col >= pivoted.columns.length) continue;
      const denseRow = underlyingRow(row);
      const cell = denseRow?.cells[col]?.cell;
      out.push({ style_json: cell?.style_json ?? '{}' });
    }
    return out;
  }

  /** Unanimity-derived props for `ShStylePanel` (Matt's ruling 4, scope
   *  report "Matt's Rulings"): `null` whenever the selection's cells
   *  disagree on that value — the panel then highlights nothing for that
   *  section, rather than guessing from e.g. just the anchor cell. */
  let panelBg = $derived.by(() => detectUnanimity(selectedStyleJsons(), 'bg'));
  let panelFg = $derived.by(() => detectUnanimity(selectedStyleJsons(), 'fg'));
  /** Border unanimity, per side, independently (#2874 item 29 —
   *  superseding the old "pick whichever ONE side is unanimous first"
   *  comment that used to live here: the panel's Top/Btm/Left/Rgt
   *  buttons are now independent toggles, so ALL non-absent sides must be
   *  reported, not just the first match, or a selection unanimously
   *  bordered on both top AND left would only ever light up "Top".
   *
   *  #2874 item 45 (Matt's ruling, 2026-09-24): `detectBorderUnanimity`
   *  now returns a three-way `BorderSideResult` per side — `'none'`,
   *  `'unanimous'`, or `'mixed'` (same side disagrees: partial presence
   *  or a w/c/dashed mismatch) — instead of collapsing `'none'` and
   *  `'mixed'` into the same falsy `null`. Matt's ruling is literal:
   *  "the side button stays lit and None and all weights stay dark" for
   *  a mixed side. `sides` below is therefore every side that is EITHER
   *  `'unanimous'` OR `'mixed'` (i.e. every side that isn't purely
   *  absent) — `ShStylePanel` lights a side's button whenever it's in
   *  `sides` and only lights "None" when `sides` is empty, so including
   *  mixed sides here is what keeps the side button lit and "None" dark
   *  for a mixed selection, with no `ShStylePanel` changes required for
   *  that half of the ruling.
   *
   *  Weight/dashed: Matt's ruling (scope report "Matt's Rulings" #1,
   *  option B, extended by item 45) — if the non-absent sides disagree
   *  with EACH OTHER, or any single side is itself `'mixed'`, NEITHER
   *  the weight nor the dashed control highlights anything
   *  (indeterminate: `w`/`dashed` come back `null`), rather than picking
   *  one side's values arbitrarily. A selection with only one bordered
   *  side that unanimously agrees can never disagree with itself, so the
   *  common single-side case is unaffected. Returns `null` — the panel's
   *  own "nothing unanimous, show sane defaults" case — both when
   *  nothing is selected AND when every side is unanimously borderless
   *  (`sides.length === 0`; a real, agreeing state: "no border", not an
   *  ambiguous one, so it must fall through to `ShStylePanel`'s
   *  null-branch defaults of weight=1/dashed=false rather than the
   *  indeterminate weight/dashed state a genuine disagreement produces). */
  let panelBorder = $derived.by(() => {
    const cells = selectedStyleJsons();
    if (cells.length === 0) return null;
    const allSides: BorderSpec['side'][] = ['top', 'bottom', 'left', 'right'];
    const activeSides: BorderSpec['side'][] = [];
    const distinctSpecs: Array<{ w: number; dashed: boolean }> = [];
    let anyMixed = false;
    for (const side of allSides) {
      const result = detectBorderUnanimity(cells, side);
      if (result.state === 'none') continue;
      activeSides.push(side);
      if (result.state === 'mixed') {
        anyMixed = true;
        continue;
      }
      if (!distinctSpecs.some((s) => s.w === result.w && s.dashed === result.dashed)) {
        distinctSpecs.push({ w: result.w, dashed: result.dashed });
      }
    }
    if (activeSides.length === 0) return null;
    const agrees = !anyMixed && distinctSpecs.length === 1;
    // #2874 item 53 (Matt's ruling, 2026-10-04): "All lights only if
    // EVERY selected cell has all four sides; otherwise individual
    // sides show as today." This is a DIFFERENT check from the
    // per-side unanimity loop above — that loop reports a side as
    // "active" the moment ANY cell in the selection has it (unanimous
    // OR mixed), so all four sides can come back active even when no
    // single cell actually has all four at once (an outline shape
    // spread across a multi-cell selection is the common case: each
    // cell only has 1-2 of the four sides, but across the whole
    // selection, all four directions are touched by SOME cell). `mode`
    // requires the stronger, per-cell check: does EVERY cell in the
    // selection individually carry all four sides. For a single-cell
    // selection the two checks can never disagree (the per-side loop's
    // "some cell has it" and "every cell has it" collapse to the same
    // cell), matching Matt's "for a single cell the two are the same
    // result" note.
    const everyCellHasAllFour = cells.every((cell) => {
      const sidesPresent = new Set((parseCellStyleRaw(cell.style_json).bd ?? []).map((b) => b.side));
      return allSides.every((s) => sidesPresent.has(s));
    });
    return {
      sides: activeSides,
      w: agrees ? distinctSpecs[0].w : null,
      dashed: agrees ? distinctSpecs[0].dashed : null,
      mode: (everyCellHasAllFour ? 'all' : 'sides') as 'all' | 'sides',
    };
  });

  /** Applies a style PATCH (fill, ink, or border) to every cell the active
   *  selection covers. For `bd`, Matt's outer-edge border ruling (scope
   *  report "Matt's Rulings" §3) is resolved HERE, per cell, against the
   *  selection's actual range shape (the only place that shape is known).
   *
   *  #2874 item 29: `patch.border.sides` is now an ARRAY — the FULL,
   *  current set of active directional toggle sides from `ShStylePanel`
   *  (not a single side, and not a delta/add-or-remove instruction).
   *  Every apply call carries the complete desired state, since
   *  `ShStylePanel`'s border state already tracks additions/removals
   *  locally and always fires `onApplyBorder` with everything currently
   *  toggled on, PLUS (#2874 item 53, see below) the tagged `mode` that
   *  decides how that side list gets painted:
   *    - `sides.length === 0`: the "none" fast path — every border on
   *      every cell in the range is cleared (`bd: null`). (Checked
   *      first, before `mode`, since an empty side list means the same
   *      thing regardless of mode.)
   *    - `mode === 'all'`: the explicit "All" fast path — painted
   *      UNCONDITIONALLY on every cell in the range, inner cells
   *      included. This is the ONLY branch that fills inner lines, and
   *      it is reached ONLY when `ShStylePanel`'s "All" button was
   *      clicked — see the item 53 note directly below for why this is
   *      no longer gated on `sides.length === 4`.
   *    - Otherwise (`mode === 'sides'`, 1–4 individual sides): this is
   *      the ONE case left that still consumes the FULL `sides` array as
   *      a snapshot — and it is reached ONLY for the single-click MODE
   *      TRANSITION out of "All" (`ShStylePanel.svelte`'s `handleSideClick`
   *      sends the full `onApplyBorder` payload, not a delta, exactly
   *      when the click that just fired also flipped `borderState.mode`
   *      away from `'all'`). Each side in the array is resolved
   *      independently via `resolvedBorderSides` against this cell's
   *      position in the range (the outer-edge rule — e.g. "top" paints
   *      border-top ONLY on cells whose row is the range's `r0`), and the
   *      resulting entries are UNIONED into a fresh `bd` — a genuine
   *      REDRAW of the whole selection as an outline, discarding
   *      whatever "All" had filled (inner lines included), per item 53's
   *      ruling ("redraws the selection as an outline"). This branch is
   *      reachable with ALL FOUR sides present too (four individual
   *      clicks starting from a borderless selection, item 53's older
   *      case) — still an outline, never a fill, because this branch is
   *      chosen by `mode`, not by counting `sides`. EVERY OTHER
   *      individual-sides-mode side click (i.e. not a transition out of
   *      "All") goes through `patch.borderSide` below instead — see that
   *      branch's doc comment and item 59's ruling for why a full-array
   *      snapshot is wrong there.
   *
   *  #2874 item 53 (Matt's ruling, 2026-10-04 — REPLACING the paragraph
   *  this one supersedes, from item 29's original fix, commit
   *  `889b88d2`/`9fb3ba3e`, PR #2923): that original paragraph said,
   *  verbatim, that the "all four sides" fast path fires whenever
   *  `sides.length === 4`, "whether the user reached this state via the
   *  'All' button or by toggling all four directional buttons
   *  individually" — explicitly treating the two as equivalent because
   *  "the resulting desired state is identical." Matt's ruling reverses
   *  that: it is NOT identical. Four individually-toggled sides draw an
   *  OUTLINE (this function's outer-edge branch, same as 1–3 sides, same
   *  as always) with "All" dark in the panel; only an EXPLICIT click on
   *  the "All" button fills inner lines and lights "All". The two cases
   *  can produce the IDENTICAL `sides` array
   *  (`['top','bottom','left','right']`) — that identical array is
   *  exactly why `sides.length === 4` can never be the right gate here;
   *  the two cases are distinguished by `patch.border.mode` (`'all' |
   *  'sides'`, a tagged value threaded from `ShStylePanel`'s own tagged
   *  `BorderPanelState` through `onApplyBorder`, not a boolean flag
   *  beside the side set — `ShStylePanel.svelte`'s `BorderPanelState`
   *  for the full rationale). THIS COMMENT INTENTIONALLY STATES THE NEW
   *  RULE, not just the diff, so nobody reads old history and restores
   *  the `sides.length === 4` equivalence believing it's still correct:
   *  it is not — that equivalence was a deliberate, documented design
   *  choice at the time, and Matt's 2026-10-04 ruling deliberately
   *  overturns it.
   *
   *  Scope note (`patch.border`'s own "sides is the full desired state"
   *  posture, now narrowed): the paragraph that used to live here said
   *  `sides` is ALWAYS the full desired state for every individual-sides
   *  click, so a directional side absent from `sides` is cleared
   *  everywhere — that was true of `patch.border` before item 59, and
   *  Matt's item 59 ruling (quoted in full below, `patch.borderSide`'s
   *  doc comment) says it is actually WRONG for the ordinary case: it
   *  deletes a cell's own stored side the user never clicked, just
   *  because that side isn't currently lit in the panel (e.g. a
   *  non-unanimous side `panelBorder`'s per-side unanimity check never
   *  seeded into the active set). `patch.border`'s full-snapshot
   *  semantics are now deliberately confined to the ONE case where a
   *  full redraw is the actually-correct behavior — the mode transition
   *  out of "All" (see this comment's `mode === 'sides'` bullet above
   *  and item 53's ruling: leaving "All" REDRAWS the selection as an
   *  outline, discarding the blanket fill, which is a real full-snapshot
   *  moment, not a per-cell data loss). Every other individual-sides
   *  click uses `patch.borderSide`'s delta instead — see its doc comment
   *  immediately below `applyStyleToSelection`'s signature for the full
   *  ruling and mechanism.
   *
   *  bg/fg patches are uniform across the whole selection (no per-cell
   *  edge logic) — `mergeStylePatch` handles those two keys unchanged.
   *
   *  Delta-audit fix (2026-09-24, `docs/audits/2026-09-24-pr-2963-
   *  item45.md` item 2 — the guard-gap Reed's audit reproduced live).
   *  Matt's ruling on the fix (authoritative, quoted in full in the fix
   *  commit): "A click in the border panel changes only what was
   *  clicked, and every cell keeps its own value for everything else.
   *  Clicking dashed on a mixed selection makes each cell dashed while
   *  keeping its own weight and color; clicking a weight works the same
   *  way for dashed. No overwrites, no dead clicks. Side-button clicks
   *  keep the wait guard as built." `patch.border` above is the
   *  "All"/"None"/leaving-"All" path (`ShStylePanel`'s `handleSideClick`)
   *  — it still writes a FULL uniform spec, which is correct for those
   *  three cases specifically: "All"/"None" are explicitly absolute per
   *  Matt's item-29 ruling, and leaving "All" is itself a full redraw per
   *  item 53. An ORDINARY individual-sides click (not one of those three)
   *  now goes through `patch.borderSide` instead, per item 59's ruling —
   *  see its own doc comment below. `patch.borderWeight`/
   *  `patch.borderDashed` below are the per-attribute paths
   *  (`handleWeightClick`/`handleDashedClick`): each carries ONLY the one
   *  attribute actually clicked plus the currently-active side set, and
   *  each cell's OWN existing entry for that side supplies every
   *  attribute NOT clicked (weight+color for a dashed click; dashed+
   *  color for a weight click) — never this panel's shared, possibly-
   *  indeterminate `borderWeight`/`borderDashed` state, which is exactly
   *  the value that produced the clobber Reed's audit reproduced (a
   *  dashed click writing `w: borderWeight ?? 1` over real, differing
   *  per-cell weights). A cell that doesn't yet have an entry for an
   *  active side gets a fresh one seeded with the sane 1px/solid default
   *  for the attribute NOT clicked (matching `ShStylePanel`'s own
   *  fallback for a brand-new border) — but ONLY when this cell sits on
   *  the selection's outer edge for that side, per the same
   *  `resolvedBorderSides` check the `patch.border` branch above already
   *  uses (Reed's second delta audit, `docs/audits/2026-09-24-pr-2963-
   *  delta2.md` item 2 — the first fix dropped this geometry check
   *  entirely, letting a weight/dashed click silently create a
   *  brand-new border on an inner, non-edge cell during a
   *  partial-presence mix, something the pre-item-29 code correctly
   *  refused to do). Matt's ruling on this fix (authoritative): "In the
   *  border panel's weight and dashed handlers, a new border may only be
   *  created on a cell that's on the selection's outer edge for that
   *  side. Use the same `resolvedBorderSides` check the full-apply path
   *  uses. Existing borders keep their own values regardless of
   *  position, as now. When a side is lit because only some cells have
   *  it, a weight or dashed click fills that border in along the outer
   *  edge, the same as clicking the side would. It never adds borders to
   *  inner cells and never changes values that weren't clicked." This
   *  gate applies ONLY to creating a fresh entry — a side that already
   *  has real per-cell data is updated per-attribute regardless of
   *  position, which is the case the ORIGINAL clobber ruling protects
   *  and remains unchanged here. */
  async function applyStyleToSelection(patch: {
    bg?: string | null;
    fg?: string | null;
    border?: { mode: 'all' | 'sides'; sides: string[]; w: number; dashed: boolean } | null;
    /** #2874 item 59 (Matt's ruling, 2026-10-07 — SUPERSEDES the item 59
     *  fix this field replaces, `docs/audits/2026-10-07-pr-2987-item59.md`,
     *  which rebuilt `bd` fresh from `sides` on every ordinary
     *  individual-sides click and was found to delete borders the user
     *  never touched — e.g. a cell's own stored Left border, vanishing
     *  the instant some OTHER cell in the selection had Top clicked,
     *  just because Left wasn't in the panel's currently-lit `sides`
     *  set). Matt's ruling (authoritative, quoted in full): "A side click
     *  changes only that side. Turning a side on adds it to cells on the
     *  selection's outer edge. Turning it off removes it from every
     *  selected cell. Every other side on every cell stays exactly as
     *  stored, whether or not it's lit in the panel." This is how the
     *  apply learns WHICH side was toggled and in WHICH direction —
     *  `ShStylePanel.svelte`'s `handleSideClick` already computes both
     *  (it diffs the clicked side against its OWN pre-click side set
     *  before updating it) and now sends them explicitly instead of
     *  collapsing them back into a full `sides` snapshot. See the
     *  `patch.borderSide` branch below for the mechanism; see
     *  `patch.border`'s own doc comment above for the one case
     *  (leaving "All" mode) that still deliberately uses a full
     *  snapshot instead of this delta. */
    borderSide?: { side: 'top' | 'bottom' | 'left' | 'right'; on: boolean; w: number; dashed: boolean };
    borderWeight?: { sides: string[]; w: number };
    borderDashed?: { sides: string[]; dashed: boolean };
  }) {
    const coords = expandSelection({ anchor: selection.anchor, range: selection.range });
    if (coords.length === 0) return;
    const range = selection.range;
    const r0 = range?.r0 ?? coords[0].row;
    const r1 = range?.r1 ?? coords[0].row;
    const c0 = range?.c0 ?? coords[0].col;
    const c1 = range?.c1 ?? coords[0].col;

    // #2874 item 51: compute every cell's per-cell style payload exactly
    // as before (the border/fill/ink merge math below is UNCHANGED, incl.
    // items 29/45/53's per-attribute resolution, the outer-edge gate, and
    // `resolvedBorderSides`) but accumulate them into ONE array and send
    // ONE `updateSheetCellsStyleBatch` call after the loop, instead of a
    // per-iteration `await api.updateSheetCell(...)` round-trip. Matt's
    // ruling: "one transaction... one recalculation for the whole batch".
    const batchEntries: { rowId: string; columnId: string; styleJson: string }[] = [];

    for (const { row, col } of coords) {
      if (row >= rowOrder.length || col >= pivoted.columns.length) continue;
      const denseRow = underlyingRow(row);
      const column = pivoted.columns[col];
      const rowId = denseRow?.row.id;
      const columnId = column?.id;
      if (!rowId || !columnId) continue;
      const cell = denseRow?.cells[col]?.cell;
      const existing = parseCellStyleRaw(cell?.style_json);

      const cellPatch: Partial<CellStyleInput> = {};
      if ('bg' in patch) cellPatch.bg = patch.bg;
      if ('fg' in patch) cellPatch.fg = patch.fg;

      if (patch.border) {
        const { mode, sides, w, dashed } = patch.border;
        if (sides.length === 0) {
          cellPatch.bd = null;
        } else if (mode === 'all') {
          // #2874 item 53: the ONLY branch that fills inner lines —
          // reached ONLY when `ShStylePanel`'s "All" button itself was
          // clicked (gated on `mode`, not on `sides.length === 4` — see
          // this function's doc comment above for why that equivalence
          // was reversed).
          const allSides: BorderSpec['side'][] = ['top', 'bottom', 'left', 'right'];
          cellPatch.bd = allSides.map((s) => ({ side: s, w, c: borderColor(), dashed }));
        } else {
          // #2874 item 59 fix (Matt's ruling, 2026-10-06 — see
          // `docs/spec/sheets.md` §33.6.1 and
          // `docs/audits/2026-10-06-2874-items-59-61.md`): `sides` IS the
          // complete desired state for this cell's border in 'sides' mode
          // (this function's own doc comment above already said so —
          // "a directional side that's absent from `sides` is cleared for
          // every cell"); the PREVIOUS code here contradicted that by
          // starting `acc` from `existingBd.filter((b) =>
          // !sides.includes(b.side))` — which KEEPS any stored entry for a
          // side that is absent from `sides`, i.e. a side that was just
          // turned off. That stale entry was then never removed (the
          // add-back loop below only iterates `sides`), so a toggled-off
          // side survived in `bd` indefinitely — reproducible with a plain
          // Top → Left → Top-off sequence on one cell (leaves `[top,
          // left]` instead of `[left]`), not limited to an All →
          // individual-sides transition. The fix: `acc` is built fresh
          // from `sides` + the outer-edge resolution ONLY — `existingBd`
          // is never consulted in this branch, because the four
          // `BorderSpec['side']` values are the only possible directions,
          // so `sides` (plus per-cell position) always fully determines
          // this cell's entire border set while in 'sides' mode. A side
          // that stays active across two clicks (e.g. `left` while `top`
          // is newly added) is unaffected by this change — it's still
          // recomputed the same way on every call, same as before; only a
          // side that just LEFT `sides` is now actually cleared instead of
          // surviving as a ghost.
          let acc: BorderSpec[] = [];
          for (const rawSide of sides) {
            const edgeSide = rawSide as BorderSpec['side'];
            const onEdge = resolvedBorderSides(edgeSide, { row, col }, { r0, r1, c0, c1 }).includes(edgeSide);
            if (onEdge) acc = [...acc, { side: edgeSide, w, c: borderColor(), dashed }];
          }
          cellPatch.bd = acc;
        }
      }

      // #2874 item 59 (Matt's ruling, 2026-10-07, quoted in full in
      // `applyStyleToSelection`'s `patch.borderSide` field doc comment
      // above — this is the mechanism that ruling requires). An ordinary
      // individual-sides click — any side toggle that is NOT the
      // mode-transition-out-of-"All" case `patch.border` above still
      // owns — changes ONLY the one side the user clicked, on THIS
      // cell's own existing `bd`, never rebuilding the rest from
      // `sides`:
      //   - Turning a side ON: if this cell is on the selection's outer
      //     edge for that side (same `resolvedBorderSides` geometry gate
      //     every other border branch in this function uses), the side's
      //     entry is replaced with the freshly-clicked `w`/`dashed`
      //     (added if absent, overwritten if a stale one somehow already
      //     existed). If this cell is NOT on that edge, `cellPatch.bd` is
      //     left UNSET entirely — not even assigned to a copy of
      //     `existingBd` — so `mergeStylePatch` below falls through to
      //     `existing.bd` verbatim, same as if `bd` were never touched
      //     this call. Either way, every side OTHER than the clicked one
      //     is never read, filtered, or rewritten here.
      //   - Turning a side OFF: the clicked side's entry is filtered out
      //     of `existingBd` on EVERY selected cell, regardless of outer-
      //     edge position (Matt's ruling: "removes it from every selected
      //     cell") — every other entry in `existingBd` passes through
      //     the filter untouched, whether or not that other side happens
      //     to be lit in the panel right now.
      // This is the direct fix for the bug this ruling describes: the
      // REBUILD approach (the code this replaces, PR #2987 at
      // `4c7a457d`) discarded `existingBd` wholesale and regenerated `bd`
      // from `sides` plus the outer-edge check on every call — so a
      // cell's own stored side that wasn't currently lit in the panel
      // (e.g. a middle cell's own Left border, in a selection where only
      // Top was ever clicked) was silently deleted the moment ANY side
      // was applied anywhere in the selection, even though the user never
      // touched that side or that cell. This branch never discards
      // `existingBd` — it only ever adds or removes the ONE side named in
      // `patch.borderSide`.
      if (patch.borderSide) {
        const { side, on, w, dashed } = patch.borderSide;
        const existingBd = existing.bd ?? [];
        if (on) {
          const onEdge = resolvedBorderSides(side, { row, col }, { r0, r1, c0, c1 }).includes(side);
          if (onEdge) {
            cellPatch.bd = [...existingBd.filter((b) => b.side !== side), { side, w, c: borderColor(), dashed }];
          }
          // Not on this side's outer edge: nothing to add, and nothing
          // else on this cell was clicked — `cellPatch.bd` stays unset so
          // `existing.bd` passes through unchanged below.
        } else {
          cellPatch.bd = existingBd.filter((b) => b.side !== side);
        }
      }

      // Delta-audit fix #2 (2026-09-24, `docs/audits/2026-09-24-pr-2963-
      // delta2.md` item 2 FAIL): per-attribute weight-only write. For
      // every side the panel currently has active, update ONLY that
      // side's `w` on THIS cell's own existing entry — `c`/`dashed` are
      // copied through unchanged from whatever this specific cell already
      // had (per Matt's original ruling: "keeping its own ...
      // color"/"its own dashed state"). A side with NO existing entry on
      // this cell gets a fresh one ONLY when this cell is on the
      // selection's outer edge for that side — the same
      // `resolvedBorderSides` check the `patch.border` branch above uses
      // (Matt's fix ruling: "a new border may only be created on a cell
      // that's on the selection's outer edge for that side ... It never
      // adds borders to inner cells"). This is what makes
      // partial-presence "fill in along the outer edge, same as clicking
      // the side would" work: an inner cell with no entry gets nothing;
      // an outer-edge cell with no entry gets a fresh one at the clicked
      // weight with the sane solid-line default (no real dashed value on
      // this cell/side to preserve, so nothing is overwritten). A cell
      // that already has a real entry for the side is updated regardless
      // of position — that carve-out is unchanged from the first fix.
      if (patch.borderWeight) {
        const { sides, w } = patch.borderWeight;
        const existingBd = existing.bd ?? [];
        const bySide = new Map(existingBd.map((b) => [b.side, b]));
        for (const rawSide of sides) {
          const s = rawSide as BorderSpec['side'];
          const current = bySide.get(s);
          if (current) {
            bySide.set(s, { ...current, w });
          } else if (resolvedBorderSides(s, { row, col }, { r0, r1, c0, c1 }).includes(s)) {
            bySide.set(s, { side: s, w, c: borderColor(), dashed: false });
          }
        }
        cellPatch.bd = [...bySide.values()];
      }

      // Mirror of the weight branch above for the dashed toggle — updates
      // ONLY `dashed` on each active side's existing entry, preserving
      // this cell's own `w`/`c`. A side with no existing entry gets a
      // fresh one, at 1px/solid-default weight with the clicked dashed
      // value, ONLY when this cell is on the selection's outer edge for
      // that side — same outer-edge gate and same rationale as the weight
      // branch above (delta-audit fix #2).
      if (patch.borderDashed) {
        const { sides, dashed } = patch.borderDashed;
        const existingBd = (cellPatch.bd as BorderSpec[] | undefined) ?? existing.bd ?? [];
        const bySide = new Map(existingBd.map((b) => [b.side, b]));
        for (const rawSide of sides) {
          const s = rawSide as BorderSpec['side'];
          const current = bySide.get(s);
          if (current) {
            bySide.set(s, { ...current, dashed });
          } else if (resolvedBorderSides(s, { row, col }, { r0, r1, c0, c1 }).includes(s)) {
            bySide.set(s, { side: s, w: 1, c: borderColor(), dashed });
          }
        }
        cellPatch.bd = [...bySide.values()];
      }

      const merged = mergeStylePatch(existing, cellPatch);
      const json = buildCellStyleJson(merged);
      batchEntries.push({ rowId, columnId, styleJson: json });
    }

    if (batchEntries.length === 0) return;
    try {
      const updated = await api.updateSheetCellsStyleBatch(noteId, batchEntries);
      applyLocalCellPatches(updated);
    } catch (e) {
      showToast(`Couldn't apply styling: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /** Border color is fixed to the design's accent-strong token for every
   *  weight/dash combination — the panel's own segment UI has no color
   *  picker for borders (only fill/ink have swatches; border styling is
   *  side/weight/dashed only, per the design), so this is the single
   *  hardcoded border color `applyStyleToSelection` writes for any BRAND
   *  NEW border entry (full-spec apply, or a per-attribute weight/dashed
   *  click landing on a side that has no prior entry on this cell yet).
   *  Takes no arguments — the color never varies by weight/dashed/side,
   *  so there was never anything to derive from its old (unused) patch
   *  parameter. */
  function borderColor(): string {
    return 'var(--sheet-border-styled)';
  }

  /** Number of cells the ACTIVE SELECTION covers (not just the
   *  right-clicked cell) — drives `ShCellContextMenu`'s "Delete N cells'
   *  contents" label. A bare anchor (no range) is 1 cell; an active range
   *  is its full rectangular cell count, matching `clearSelectedContents`/
   *  `clearSelectedStyling`'s own cell-iteration below. */
  let selectionCellCount = $derived.by(() => {
    if (selection.range) {
      const r = selection.range;
      return (r.r1 - r.r0 + 1) * (r.c1 - r.c0 + 1);
    }
    return selection.anchor ? 1 : 0;
  });

  function handleCellContextMenu(row: number, col: number, e: MouseEvent) {
    e.preventDefault();
    const coord: CellCoord = { row, col };
    const inSelection = isInRange(coord, selection.range) || isAnchor(coord, selection.anchor);
    if (!inSelection) {
      dispatch({ type: 'SELECT_CELL', coord });
    }
    contextMenuCell = { row, col };
    contextMenuPos = { x: e.clientX, y: e.clientY };
  }

  function closeContextMenu() {
    contextMenuCell = null;
    contextMenuPos = null;
  }

  /** Every (row, col) coordinate the active selection covers — the anchor
   *  alone if no range is active, every cell in the range otherwise. The
   *  SHARED walk both `clearSelectedContents` and `clearSelectedStyling`
   *  use, so the two verbs can never diverge on which cells they touch
   *  (scope report §6, "Two-doors consistency"). Ghost coordinates (past
   *  the real row/column extent) are silently skipped — there is no
   *  underlying cell to clear. */
  function selectedCellCoords(): CellCoord[] {
    const coords: CellCoord[] = [];
    if (selection.range) {
      const r = selection.range;
      for (let row = r.r0; row <= r.r1; row++) {
        for (let col = r.c0; col <= r.c1; col++) {
          if (row < rowOrder.length && col < pivoted.columns.length) coords.push({ row, col });
        }
      }
    } else if (selection.anchor) {
      const { row, col } = selection.anchor;
      if (row < rowOrder.length && col < pivoted.columns.length) coords.push({ row, col });
    }
    return coords;
  }

  /** "Delete N cells' contents" (context-menu row) AND the Delete/
   *  Backspace keyboard shortcuts (Matt's ruling 2026-09-05 #2: bind as
   *  keyboard shortcuts ALONGSIDE the menu row, a ruled addition to the
   *  design's menu-only depiction). Clears VALUES only — `style_json` is
   *  never touched ("clears contents only — values, not styling", same
   *  ruling). Uses the plain `update_sheet_cell(rowId, columnId, '')`
   *  path (no explicit `styleJson` arg — `api.updateSheetCell`'s own
   *  default already omits it, leaving the backend's existing style_json
   *  untouched, matching `update_sheet_cell_internal`'s independent-
   *  Option contract, scope report §3.8). Only cells that currently HAVE a
   *  value are written — clearing an already-empty cell is a no-op, not a
   *  wasted round-trip. Loops sequentially (scope report §3.3's accepted
   *  non-atomic posture, same as `set_sheet_cells`'s own documented
   *  contract) — no batch command exists or is being added by this PR. */
  async function clearSelectedContents() {
    const coords = selectedCellCoords();
    for (const { row, col } of coords) {
      const denseRow = underlyingRow(row);
      const column = pivoted.columns[col];
      const rowId = denseRow?.row.id;
      const columnId = column?.id;
      if (!rowId || !columnId) continue;
      const cell = denseRow?.cells[col]?.cell;
      if (!cell || !cell.raw_value) continue; // nothing to clear
      try {
        const updated = await api.updateSheetCell(rowId, columnId, '');
        applyLocalCellPatch(rowId, columnId, updated);
      } catch (e) {
        showToast(`Couldn't clear cell contents: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    if (sheetHasFormulas()) await refetchSheetData();
  }

  /** "Clear styling" (context-menu row only — no keyboard shortcut; Matt's
   *  ruling: "Clear styling remains its own verb"). Clears `style_json`
   *  ONLY — the cell's value is left untouched. Calls
   *  `updateSheetCell(rowId, colId, null, '{}')` per cell: `value: null`
   *  falls back to the cell's existing value on the backend
   *  (`update_sheet_cell_internal`'s independent-Option contract, scope
   *  report §3.8 — already covered by an existing backend test), and
   *  `styleJson: '{}'` is an explicit "no styling" write (not `null`,
   *  which would mean "leave style_json as-is"). This is the SECOND
   *  style-only write path (alongside `applyStyleToSelection`) — as of
   *  #2874 item 47, the backend's reply for a style-only write always
   *  carries the cell's current computed value (and its A1-translated
   *  formula), exactly like a normal read, so the patch below is a
   *  direct trust-the-reply write with no client-side preserve/guess
   *  logic needed. */
  async function clearSelectedStyling() {
    const coords = selectedCellCoords();
    // #2874 item 51: same batching as `applyStyleToSelection` — compute
    // the "nothing to clear" skip per cell exactly as before, but send
    // every cell that DOES need clearing in ONE `updateSheetCellsStyleBatch`
    // call instead of one `updateSheetCell` round-trip per cell.
    const batchEntries: { rowId: string; columnId: string; styleJson: string }[] = [];
    for (const { row, col } of coords) {
      const denseRow = underlyingRow(row);
      const column = pivoted.columns[col];
      const rowId = denseRow?.row.id;
      const columnId = column?.id;
      if (!rowId || !columnId) continue;
      const cell = denseRow?.cells[col]?.cell;
      if (!cell || !cell.style_json || cell.style_json === '{}') continue; // nothing to clear
      batchEntries.push({ rowId, columnId, styleJson: '{}' });
    }

    if (batchEntries.length === 0) return;
    try {
      const updated = await api.updateSheetCellsStyleBatch(noteId, batchEntries);
      applyLocalCellPatches(updated);
    } catch (e) {
      showToast(`Couldn't clear cell styling: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  function mismatchExplanation(colType: string): string {
    switch (colType) {
      case 'number': return 'Not a number';
      case 'currency': return 'Not a number';
      case 'date': return 'Not a date';
      case 'checkbox': return 'Not a checkbox value';
      case 'select': return 'Not one of this column\u2019s choices';
      default: return 'Doesn\u2019t match this column\u2019s type';
    }
  }

  // ── Column & row resize (#2339 PR 13, ShResize, §33.3.3) ────────────
  //
  // Drag-time state only — nothing in `pivoted`/`sheetData` changes until
  // mouseup commits ("nothing else moves until the drag ends", design Cap
  // prose). Grip elements (`.sh-h__grip`/`.sh-g__grip`) live directly in
  // this component's template (no separate ShResize/SheetViewStrip-style
  // component — scope report §7.1) and attach `onmousedown` handlers that
  // call `startColumnResize`/`startRowResize` below; `window`-level
  // mousemove/mouseup listeners (not element-level) track the drag past
  // the grip's own bounds and even past the grid entirely (scope §7.4).

  interface ResizeState {
    axis: 'col' | 'row';
    /** Column index (into `pivoted.columns`) or DISPLAY row index (into
     *  `visibleRows`), matching how every other handler in this file
     *  addresses rows/columns. */
    index: number;
    /** `clientX` (col) or `clientY` (row) at drag start. */
    startPos: number;
    /** The column's `effectiveWidth` / row's `height` at drag start. */
    startSize: number;
    /** Live-computed during drag, clamped to the axis's floor — drives the
     *  guide position and readout text. The actual grid does NOT re-render
     *  at this size until commit. */
    currentSize: number;
  }
  let resizeState: ResizeState | null = $state(null);
  /** Latest cursor position during an active resize drag — drives the
   *  fixed-position readout chip (`MouseEvent.clientX/clientY` + a small
   *  fixed offset, per Matt's ruling 2026-09-04: no direction flip, only
   *  viewport-edge clamping). */
  let resizeCursorX = $state(0);
  let resizeCursorY = $state(0);

  const READOUT_OFFSET_PX = 8;
  /** Rough size estimate for the readout chip, used only for viewport-edge
   *  clamping math (ports `ShHeaderMenu`'s boundary-flip pattern, scope
   *  report §4.4 — the design frame is silent on edge behavior, this is
   *  the sanctioned improvisation). The chip's actual rendered size varies
   *  slightly with digit count ("40 px" vs "1024 px") but never enough to
   *  matter for a clamp check. */
  const READOUT_EST_WIDTH = 72;
  const READOUT_EST_HEIGHT = 26;

  function startColumnResize(ci: number, e: MouseEvent) {
    e.stopPropagation();
    const column = pivoted.columns[ci];
    if (!column) return;
    resizeState = { axis: 'col', index: ci, startPos: e.clientX, startSize: column.effectiveWidth, currentSize: column.effectiveWidth };
    resizeCursorX = e.clientX;
    resizeCursorY = e.clientY;
    window.addEventListener('mousemove', handleResizeMouseMove);
    window.addEventListener('mouseup', handleResizeMouseUp);
  }

  function startRowResize(ri: number, e: MouseEvent) {
    e.stopPropagation();
    const denseRow = visibleRows[ri];
    if (!denseRow) return;
    resizeState = { axis: 'row', index: ri, startPos: e.clientY, startSize: denseRow.height, currentSize: denseRow.height };
    resizeCursorX = e.clientX;
    resizeCursorY = e.clientY;
    window.addEventListener('mousemove', handleResizeMouseMove);
    window.addEventListener('mouseup', handleResizeMouseUp);
  }

  function handleResizeMouseMove(e: MouseEvent) {
    if (!resizeState) return;
    const floor = resizeState.axis === 'col' ? MIN_COLUMN_WIDTH : MIN_ROW_HEIGHT;
    const delta = resizeState.axis === 'col' ? e.clientX - resizeState.startPos : e.clientY - resizeState.startPos;
    resizeState = { ...resizeState, currentSize: computeNewSize(resizeState.startSize, delta, floor) };
    resizeCursorX = e.clientX;
    resizeCursorY = e.clientY;
  }

  /** Commit-on-mouseup (scope §5.3/§3.5): a zero-delta drag (grip clicked
   *  without moving) skips the API call entirely — no wasted no-op write,
   *  no snapshot capture, no refetch. A non-zero delta commits via the
   *  existing `update_sheet_column` width path (columns) or the new
   *  `update_sheet_row` height path (rows), then refetches so the grid
   *  re-pivots at the new size. Write failure → `showToast`, matching
   *  every other mutating call site in this file. */
  async function handleResizeMouseUp() {
    window.removeEventListener('mousemove', handleResizeMouseMove);
    window.removeEventListener('mouseup', handleResizeMouseUp);
    if (!resizeState) return;
    const snapshot = resizeState;
    resizeState = null;
    if (snapshot.currentSize === snapshot.startSize) return;

    try {
      if (snapshot.axis === 'col') {
        const column = pivoted.columns[snapshot.index];
        if (!column) return;
        await api.updateSheetColumn(column.id, { width: snapshot.currentSize });
      } else {
        const denseRow = visibleRows[snapshot.index];
        if (!denseRow) return;
        await api.updateSheetRow(denseRow.row.id, snapshot.currentSize);
      }
      await refetchSheetData();
    } catch (e) {
      const what = snapshot.axis === 'col' ? 'column' : 'row';
      showToast(`Couldn't resize ${what}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /** Live pixel position (within `.sh-gridwrap`'s content coordinate
   *  space) of the boundary being dragged — drives `.sh-guide`'s `left`
   *  (col) or `top` (row). Sums effective widths/heights up to (and
   *  including, at its LIVE `currentSize`) the boundary being resized;
   *  every other column/row uses its unchanged effective size, since
   *  nothing else moves during the drag. */
  let resizeGuidePos = $derived.by(() => {
    if (!resizeState) return 0;
    if (resizeState.axis === 'col') {
      let x = GUTTER_W;
      for (let i = 0; i < pivoted.columns.length; i++) {
        if (i === resizeState.index) {
          x += resizeState.currentSize;
          break;
        }
        x += pivoted.columns[i].effectiveWidth;
      }
      return x;
    }
    let y = HEAD_H;
    for (let i = 0; i < visibleRows.length; i++) {
      if (i === resizeState.index) {
        y += resizeState.currentSize;
        break;
      }
      y += visibleRows[i].height;
    }
    return y;
  });

  /** Viewport-clamped `position: fixed` coordinates for the readout chip
   *  (scope §4.4 — ports `ShHeaderMenu`'s own boundary-flip pattern since
   *  the design frame shows no edge-of-screen example). Fixed offset from
   *  the cursor, no direction flip (Matt's ruling 2026-09-04) — only
   *  clamped inward when the naive offset position would overflow the
   *  viewport's right/bottom edge. */
  let resizeReadoutPos = $derived.by(() => {
    let left = resizeCursorX + READOUT_OFFSET_PX;
    let top = resizeCursorY + READOUT_OFFSET_PX;
    if (typeof window !== 'undefined') {
      const vw = (window as any).innerWidth ?? Infinity;
      const vh = (window as any).innerHeight ?? Infinity;
      if (left + READOUT_EST_WIDTH > vw) left = Math.max(0, vw - READOUT_EST_WIDTH);
      if (top + READOUT_EST_HEIGHT > vh) top = Math.max(0, vh - READOUT_EST_HEIGHT);
    }
    return { left, top };
  });

  onDestroy(() => {
    window.removeEventListener('mousemove', handleResizeMouseMove);
    window.removeEventListener('mouseup', handleResizeMouseUp);
    // #2874 item 19: same leak-guard as the resize listeners above — if
    // the component unmounts mid-drag (e.g. navigating away from the
    // sheet while a drag-select is in progress) the window-level
    // mouseup listener `handleCellMouseDown` attached would otherwise
    // outlive this component instance. Removing an already-removed
    // listener (the common case, drag finished normally) is a no-op.
    window.removeEventListener('mouseup', handleDragSelectMouseUp);
    unlistenDataChanged?.then(fn => fn());
  });

  // ── Toast (ghost-commit / checkbox / rename failure feedback, ruling 8) ─

  let toastMsg = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  function showToast(msg: string) {
    toastMsg = msg;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toastMsg = null;
    }, 5000);
  }

  function dismissToast() {
    toastMsg = null;
    if (toastTimer) clearTimeout(toastTimer);
  }

  /** `rowCount` here is the DISPLAY row count — `rowOrder.length`, i.e.
   *  the post-sort/filter visible real-row count, not `pivoted.rows.
   *  length` — so keyboard-nav/ghost-row math ("where does the first
   *  ghost row start?") is computed against what's actually on screen.
   *  With no active filter this equals `pivoted.rows.length` exactly
   *  (identity permutation, ruling: sort/filter never change the ROW
   *  COUNT on their own — only filter does, by design). */
  function bounds(): SheetBounds {
    return {
      rowCount: rowOrder.length,
      colCount: pivoted.columns.length,
      ghostRowCount: GHOST_ROWS,
      ghostColCount: GHOST_COLS,
    };
  }

  // ── Formula bar feed + dashed ref outlines (#2339 PR 14) ────────────

  /** A1 address for ANY grid coordinate, real or ghost — `colRef` (defined
   *  further below) is purely arithmetic so it already works past the
   *  real column count; the row half mirrors the real-row-gutter's own
   *  `rowOrder[ri] + 1` (stored position, not display position) vs. the
   *  ghost-row-gutter's `gRow + 1` (sequential), matching whichever of the
   *  two the coordinate falls into. */
  function a1RefFor(row: number, col: number): string {
    const rowNumber = row < rowOrder.length ? rowOrder[row] + 1 : row + 1;
    return colRef(col) + rowNumber;
  }

  /** Everything `FormulaBar.svelte` (mounted in Editor.svelte, outside
   *  this component) needs to render for the current anchor cell —
   *  recomputed on every selection/editing/error change and pushed via
   *  `onSelectionInfo` in the `$effect` below. "One formula, two places to
   *  edit it" (design Cap prose): this and the in-cell editor both read/
   *  write the SAME `editing.value` buffer, so a bar-driven edit and a
   *  cell-driven edit of the anchor are indistinguishable to the rest of
   *  this component. */
  let formulaBarInfo: FormulaBarInfo = $derived.by(() => {
    const anchor = selection.anchor;
    if (!anchor) return { cref: '', value: '', editing: false, error: null, editable: false, selectionStart: null, selectionEnd: null, selectedColumnIndex: null, selectedColumnType: null };
    const { row, col } = anchor;
    const isGhostRow = row >= rowOrder.length;
    const isGhostCol = col >= pivoted.columns.length;
    const column = isGhostCol ? undefined : pivoted.columns[col];
    const colType = column?.col_type;
    // Ruling 2: checkbox/select have no typed-buffer formula path — the
    // bar has nothing sensible to drive for those two column types.
    const editable = colType !== 'checkbox' && colType !== 'select';
    const isEditingThisCell = editing !== null && editing.row === row && editing.col === col;
    let value = '';
    if (isEditingThisCell && editing) {
      value = editing.value;
    } else if (!isGhostRow && !isGhostCol) {
      const denseRow = underlyingRow(row);
      const cell = denseRow?.cells[col]?.cell ?? null;
      value = cell?.raw_value ?? '';
    }
    return {
      cref: a1RefFor(row, col),
      value,
      editing: isEditingThisCell,
      error: isEditingThisCell ? editorError : null,
      editable,
      // #2339 PR 14 fix (bar/cell caret parity): only meaningful while
      // THIS cell is the one being edited — otherwise there's no live
      // caret to report at all.
      selectionStart: isEditingThisCell && editing ? editing.selectionStart : null,
      selectionEnd: isEditingThisCell && editing ? editing.selectionEnd : null,
      // #2874 item 16 Part C: `colSel ?? anchor.col` — the same
      // forgiving-source resolution `toolbarSort`/`toolbarFilter` apply,
      // computed here too so `Editor.svelte` can gate its toolbar props
      // off this single feed. A resolved index past the real columns
      // (shouldn't happen since `col` is always a real/ghost coordinate
      // already bounds-checked above via `isGhostCol`) reports `null`.
      selectedColumnIndex: (selection.colSel ?? col) < pivoted.columns.length ? (selection.colSel ?? col) : null,
      selectedColumnType: (selection.colSel ?? col) < pivoted.columns.length ? (pivoted.columns[selection.colSel ?? col]?.col_type ?? null) : null,
    };
  });

  $effect(() => {
    onSelectionInfo?.(formulaBarInfo);
  });

  /** Every A1 range the CURRENTLY-EDITING formula buffer references,
   *  resolved against the REAL (sort_order) grid — drives the dashed
   *  cyan `.sh-c.is-ref`/`.sh-c.is-refedge-*` outline while a formula is
   *  being edited (design annotation: "every range it references gets a
   *  dashed cyan outline… dashed, because the solid outline already means
   *  'this is your selection'"). Empty whenever nothing is being edited,
   *  or the buffer isn't a formula (doesn't start with `=`) — a typed
   *  value never gets ref outlines. */
  let formulaEditRanges: FormulaRefRange[] = $derived.by(() => {
    if (!editing || !editing.value.startsWith('=')) return [];
    return extractFormulaRefRanges(editing.value);
  });

  /** Dashed-edge classes for the REAL cell at display coordinate
   *  `(ri, ci)`, or `null` if it's outside every referenced range (no
   *  outline at all) — converts the DISPLAY row index to its underlying
   *  (sort_order-equivalent) row index via `rowOrder` before checking,
   *  since `formulaEditRanges` is expressed in REAL grid coordinates, not
   *  display coordinates (the two differ under an active sort). */
  function cellRefEdges(ri: number, ci: number): { top: boolean; bottom: boolean; left: boolean; right: boolean } | null {
    if (formulaEditRanges.length === 0) return null;
    const underlyingIdx = rowOrder[ri];
    if (underlyingIdx === undefined) return null;
    if (!isCellInAnyRefRange(ci, underlyingIdx, formulaEditRanges)) return null;
    return refRangeEdges(ci, underlyingIdx, formulaEditRanges);
  }

  // ── Formula bar imperative API (bind:this from Editor.svelte) ───────

  /** Opens the SAME editor `handleCellDblClick` would (existing value
   *  preserved, not cleared) for the current anchor cell — the entry point
   *  for "click into the formula bar and start typing" when the anchor
   *  cell isn't already mid-edit. No-op for checkbox/select anchors
   *  (ruling 2) or when nothing is selected. */
  export function barBeginEdit() {
    const anchor = selection.anchor;
    if (!anchor) return;
    const { row, col } = anchor;
    if (editing && editing.row === row && editing.col === col) return;
    if (row >= rowOrder.length || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (!column || column.col_type === 'checkbox' || column.col_type === 'select') return;
    // #2874 item 12 Part D: mark this session bar-originated so the
    // in-cell editor that also mounts for the same cell ("one formula, two
    // places to edit it") knows to defer its own focus() to the bar's.
    openEditorForCell(row, col, { origin: 'bar' });
  }

  /** Toolbar fx button (`Editor.svelte`'s `sheetGridRef.
   *  openFormulaOnAnchor()`) — opens the SAME editor `barBeginEdit`
   *  opens, but with `=` PREFILLED (`clearWithChar: '='`) instead of the
   *  existing value preserved, matching the "typing `=` into a cell
   *  overwrites it and starts a formula" type-to-overwrite convention
   *  already used everywhere else in this file — zero new editor
   *  behavior, just a second way to trigger the exact same path. No-op
   *  for checkbox/select anchors (ruling 2, same guard `barBeginEdit`
   *  applies) or when nothing is selected. Always starts a FRESH edit
   *  (overwriting whatever was there) even if the anchor happens to
   *  already be mid-edit — unlike `barBeginEdit`, which no-ops in that
   *  case since it's meant to resume, not replace. */
  export function openFormulaOnAnchor() {
    const anchor = selection.anchor;
    if (!anchor) return;
    const { row, col } = anchor;
    if (row >= rowOrder.length || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (!column || column.col_type === 'checkbox' || column.col_type === 'select') return;
    openEditorForCell(row, col, { clearWithChar: '=', origin: 'bar' });
  }

  /** Applies a bar-driven keystroke to the anchor cell's edit buffer,
   *  starting an edit first (via `barBeginEdit`) if one isn't already
   *  open. Mirrors the in-cell editors' own `bind:value` — this is the
   *  other end of the SAME buffer. */
  export function barSetValue(value: string) {
    if (!editing) barBeginEdit();
    if (editing) editing.value = value;
  }

  /** #2339 PR 14 fix (bar/cell caret parity): records the bar's (or the
   *  in-cell editor's, via `setEditingSelection` below) live caret/
   *  selection onto the shared `editing` session, so whichever surface
   *  gains focus next can restore it instead of always jumping to
   *  end-of-text. No-op if nothing is being edited (a stray event from an
   *  already-closed editor). */
  export function barSetSelection(start: number, end: number) {
    if (!editing) return;
    editing.selectionStart = start;
    editing.selectionEnd = end;
  }

  /** Same as `barSetSelection`, for the in-cell formula editor's own
   *  `onSelectionChange` — both surfaces write the SAME `editing` session
   *  field, this is just the other caller's name for symmetry/clarity at
   *  each call site. */
  function setEditingSelection(start: number, end: number) {
    barSetSelection(start, end);
  }

  /** Commits the anchor cell's edit buffer from the bar — delegates to the
   *  exact same `handleEditorCommit` the in-cell editors use, so bar
   *  commits get identical optimistic-close / reject-and-reopen / inline-
   *  error behavior (ruling 3/8: out-of-bounds and parse rejections surface
   *  the same way regardless of which surface triggered the write). */
  export function barCommit(cause: 'enter' | 'blur') {
    handleEditorCommit(cause);
  }

  /** Cancels a bar-driven edit — delegates to the same revert-to-pre-edit
   *  behavior the in-cell editors' Escape key uses. */
  export function barCancel() {
    handleEditorCancel();
  }

  async function loadSheet() {
    loading = true;
    loadError = null;
    try {
      const data = await api.getSheetData(noteId);
      // #2874 item 18 fix 2: intentional, early validation instead of
      // relying on an accidental TypeError from the destructure/spread
      // below (data.columns on null, or [...undefined] inside
      // pivotSheetData) to trip this same catch block. Produces a clean,
      // human-readable loadError message instead of raw JS error text.
      if (!data || !Array.isArray(data.columns) || !Array.isArray(data.rows) || !Array.isArray(data.cells)) {
        throw new Error('Sheet data is missing or malformed');
      }
      sheetData = data;
      pivoted = pivotSheetData(data.columns, data.rows, data.cells);
      selection = INITIAL_SHEET_SELECTION;
      editing = null;
      headerRename = null;
      onLoaded?.(data);
    } catch (e) {
      console.error('[SheetGrid] Failed to load sheet data:', e);
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  /** Refetch after a mutation WITHOUT resetting selection/editing state
   *  (unlike `loadSheet`, which is the initial/note-switch load and
   *  intentionally resets to A1). Used after formula-affecting commits and
   *  structural (ghost-commit) mutations, per scope §3/§4. */
  async function refetchSheetData(): Promise<void> {
    const data = await api.getSheetData(noteId);
    sheetData = data;
    pivoted = pivotSheetData(data.columns, data.rows, data.cells);
    onLoaded?.(data);
  }

  /** #2339 PR 15b-beta: public reload hook for `Editor.svelte` to call
   *  after a sheet-native version restore (`ShVersionHistoryPanel`'s
   *  `onRestored`). Wraps `refetchSheetData` (not `loadSheet`) — a restore
   *  changes cell/column data but the user's current selection/scroll
   *  position in the grid is still meaningful, unlike the initial
   *  note-switch load where resetting to A1 is correct. */
  export async function reload(): Promise<void> {
    await refetchSheetData();
  }

  // #2972 stage 2 (§34.5): SheetGrid has zero refresh listeners today
  // (confirmed at spec time — no `pane-data-changed`/`pane-note-changed`/
  // `listen(` hits in this file). Gains a single pane-data-changed
  // handler, filtered to its own `noteId`: on a match, re-reads the
  // sheet's grid data and patches in place via the existing
  // `refetchSheetData()` (cell/row/column diff via `pivotSheetData`, not a
  // full component remount) — never `loadSheet()`, which resets
  // selection/editing state. Closes the "open sheet never updates while an
  // agent edits it" gap named in both the scope doc and the #2970
  // `mutating_tools` comment.
  let unlistenDataChanged: Promise<() => void> | null = null;

  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type } = payload;
    if (entity_type !== ENTITY_SHEET) return;
    if (entity_id !== noteId) return;
    try {
      await refetchSheetData();
    } catch (e) {
      console.error('[SheetGrid] Failed to refetch sheet data for pane-data-changed event:', entity_id, e);
    }
  }

  onMount(() => {
    loadSheet();
    // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
    // backend `listen()` AND the app `CustomEvent`.
    unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });
  });

  // Reload if the host swaps to a different sheet note (e.g. Editor.svelte
  // is keyed per-note, but SheetGrid itself is only ever mounted once per
  // note in current host usage — this effect is a defensive belt-and-
  // suspenders for any future host that reuses the instance).
  let mountedNoteId = $state('');
  $effect(() => {
    if (noteId !== mountedNoteId) {
      const isFirstLoad = mountedNoteId === '';
      mountedNoteId = noteId;
      if (!isFirstLoad) loadSheet();
    }
  });

  function dispatch(action: Parameters<typeof reduceSheetSelection>[1]) {
    selection = reduceSheetSelection(selection, action, bounds());
  }

  // ── Local optimistic patching ────────────────────────────────────────

  /** Patches one cell into `sheetData`/`pivoted` in place (no refetch) —
   *  used for the common non-formula-sheet commit path (§3: "optimistic
   *  local patch, no refetch, except when the sheet contains any formula
   *  cells at all"). Does not touch `selection`. */
  function applyLocalCellPatch(rowId: string, columnId: string, updatedCell: SheetCell) {
    if (!sheetData) return;
    const idx = sheetData.cells.findIndex((c) => c.row_id === rowId && c.column_id === columnId);
    const cells = [...sheetData.cells];
    if (idx >= 0) cells[idx] = updatedCell;
    else cells.push(updatedCell);
    sheetData = { ...sheetData, cells };
    pivoted = pivotSheetData(sheetData.columns, sheetData.rows, cells);
  }

  /** Batch sibling of `applyLocalCellPatch` (#2874 item 51) — patches N
   *  updated cells into `sheetData`/`pivoted` in ONE pass (one array copy,
   *  one `pivotSheetData` rebuild) instead of looping `applyLocalCellPatch`
   *  N times, which would rebuild `cells`/`pivoted` N times for a single
   *  batched backend call. Used by `applyStyleToSelection`/
   *  `clearSelectedStyling` after their one `updateSheetCellsStyleBatch`
   *  call returns. */
  function applyLocalCellPatches(updatedCells: SheetCell[]) {
    if (!sheetData || updatedCells.length === 0) return;
    const cells = [...sheetData.cells];
    for (const updatedCell of updatedCells) {
      const idx = cells.findIndex((c) => c.row_id === updatedCell.row_id && c.column_id === updatedCell.column_id);
      if (idx >= 0) cells[idx] = updatedCell;
      else cells.push(updatedCell);
    }
    sheetData = { ...sheetData, cells };
    pivoted = pivotSheetData(sheetData.columns, sheetData.rows, cells);
  }

  function sheetHasFormulas(): boolean {
    return !!sheetData && sheetData.cells.some((c) => (c.raw_value ?? '').startsWith('='));
  }

  /** Commits one cell's value via `update_sheet_cell`, then either patches
   *  locally (formula-free sheet) or refetches (sheet contains any formula
   *  cell, per §3 — a plain-value edit can still change another cell's
   *  computed_value through a formula dependency). Throws on backend
   *  rejection (invalid formula parse, network error) — callers decide how
   *  to surface that. */
  async function commitCellValue(row: number, col: number, rawValueToStore: string): Promise<SheetCell> {
    const column = pivoted.columns[col];
    const denseRow = underlyingRow(row);
    const rowId = denseRow?.row.id;
    const columnId = column?.id;
    if (!rowId || !columnId) throw new Error('Cell no longer exists');

    const hadFormulas = sheetHasFormulas();
    const updated = await api.updateSheetCell(rowId, columnId, rawValueToStore, null);
    applyLocalCellPatch(rowId, columnId, updated);
    if (hadFormulas || rawValueToStore.startsWith('=')) {
      await refetchSheetData();
    }
    return updated;
  }

  // ── Selection / click handlers ──────────────────────────────────────

  function handleCellMouseDown(row: number, col: number, e: MouseEvent) {
    if (e.button !== 0) return;
    if (e.shiftKey) {
      dispatch({ type: 'EXTEND_TO', coord: { row, col } });
    } else {
      dispatch({ type: 'SELECT_CELL', coord: { row, col } });
    }
    isDragSelecting = true;
    // #2874 item 19: a WINDOW-level listener (not the element-scoped
    // `onmouseup` this replaced), matching `startColumnResize`/
    // `startRowResize`'s own drag pattern just above — a mouseup that
    // happens outside `.sh-gridwrap` (released over the toolbar, a
    // popover, or even outside the browser window entirely) never
    // reaches an element-scoped listener, which is exactly how a drag
    // select could get stuck "on" forever (#2874 item 19 diagnosis).
    // Attached fresh on every mousedown and torn down by
    // `handleDragSelectMouseUp` itself the first time it fires, so at
    // most one listener is ever registered at a time.
    window.addEventListener('mouseup', handleDragSelectMouseUp);
  }

  /** Click (after mousedown already selected the cell) — the ONLY thing
   *  left to do here is the checkbox toggle (ruling: "Space or click
   *  toggles in place, no edit mode"). Selection itself is handled by
   *  mousedown so this never double-dispatches. */
  function handleCellClick(row: number, col: number, e: MouseEvent) {
    if (e.shiftKey) return;
    maybeToggleCheckboxOnClick(row, col);
  }

  function handleCellDblClick(row: number, col: number) {
    // Ghost coordinates have no existing value to edit in place — PR 10
    // only wires ghost-cell activation through typing-to-commit (§4), not
    // a double-click path (no design frame shows one).
    if (row >= rowOrder.length || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (!column) return;
    if (column.col_type === 'checkbox') {
      // No third toggle here — a native double-click already delivers two
      // `click` events (each independently toggling via
      // `maybeToggleCheckboxOnClick`), which nets back to the original
      // value, matching ordinary checkbox double-click behavior. Adding a
      // dblclick-triggered toggle on top would make double-click land on
      // the OPPOSITE value from a single click, which is surprising.
      return;
    }
    if (column.col_type === 'select') {
      openSelectEditor(row, col);
      return;
    }
    openEditorForCell(row, col, {});
  }

  function handleCellMouseEnter(row: number, col: number) {
    if (isDragSelecting) {
      dispatch({ type: 'EXTEND_TO', coord: { row, col } });
    }
  }

  /** #2874 item 19: window-level mouseup handler for the drag-select
   *  gesture `handleCellMouseDown` starts — removes itself immediately
   *  (mirrors `handleResizeMouseUp`'s own remove-then-act shape) so a
   *  fresh listener is always the one doing the work, never a stale one
   *  left over from an earlier drag. Replaces the old element-scoped
   *  `onmouseup={handleGridMouseUp}` on `.sh-gridwrap`, which never fired
   *  — and left `isDragSelecting` stuck `true` — for a mouseup released
   *  outside the grid element (over the toolbar, a popover, or outside
   *  the window). */
  function handleDragSelectMouseUp() {
    window.removeEventListener('mouseup', handleDragSelectMouseUp);
    isDragSelecting = false;
  }

  // §33.3.6 (PR #2924, commit e7e341ce): left-click on a column header
  // selects the whole column and opens nothing. Right-click
  // (`handleColumnHeaderContextMenu`, below) opens the header menu.
  // Supersedes the Sep 4 one-click ruling ("one click, both effects").
  function handleColumnHeaderClick(col: number) {
    dispatch({ type: 'SELECT_COLUMN', col });
  }

  // §33.3.6 (PR #2924, commit e7e341ce): right-click on a column header
  // opens ShHeaderMenu (Rename, Type, Sort asc/desc, Filter, Delete
  // column), anchored to the clicked header. Right-click always opens (or
  // re-anchors) the menu for that column — closing happens via the
  // existing `onClose={closeHeaderMenu}` outside-click dismiss.
  function handleColumnHeaderContextMenu(col: number, e: MouseEvent) {
    e.preventDefault();
    headerMenuAnchor = e.currentTarget as HTMLElement;
    openHeaderMenu = col;
  }

  function closeHeaderMenu() {
    openHeaderMenu = null;
    headerMenuAnchor = null;
  }

  // ── ShHeaderMenu verb handlers (#2339 PR 11) ────────────────────────

  function menuRename(col: number) {
    startHeaderRename(col);
  }

  async function menuTypeChange(col: number, colType: string) {
    const column = pivoted.columns[col];
    if (!column) return;
    try {
      await api.updateSheetColumn(column.id, { colType });
      await refetchSheetData();
    } catch (e) {
      showToast(`Couldn't change column type: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  function menuSort(col: number, direction: 'asc' | 'desc') {
    const column = pivoted.columns[col];
    if (!column) return;
    // Ruling 5: single-column sort only — setting a new sort column
    // REPLACES whatever was previously sorted (no accumulation/multi-sort
    // pre-shaping).
    viewState = { ...viewState, sortColumnId: column.id, sortDirection: direction };
  }

  function menuFilterChange(col: number, values: string[]) {
    const column = pivoted.columns[col];
    if (!column) return;
    viewState = { ...viewState, filterColumnId: column.id, filterValues: values.length > 0 ? values : null };
  }

  // ── Toolbar sort/filter/formula wiring (#2874 item 16 Part C) ───────
  //
  // The column the toolbar's Sort/Filter buttons act on — “the whole
  // column, if one is header-selected; otherwise the anchor cell's
  // column” (Matt's forgiving-source ruling: `colSel ?? anchor.col`).
  // `Editor.svelte` reads this off `formulaBarInfo.selectedColumnIndex`/
  // `.selectedColumnType` (extended below, alongside `formulaBarInfo`'s
  // existing fields) rather than a second dedicated export — one
  // reactive feed for every piece of “what's currently selected” state
  // Editor.svelte needs, matching the existing `onSelectionInfo` wiring's
  // own “single feed” discipline.

  /** Toolbar Sort button (`Editor.svelte`'s `sheetGridRef.toolbarSort()`)
   *  — resolves the target column via `selectedColumnIndex` and CYCLES the
   *  sort on successive presses: ascending → descending → cleared (Matt's
   *  ruling, #2874 item 16). Current state is read straight off
   *  `viewState` (the same single source of truth `ShHeaderMenu`'s Sort
   *  rows and the header-flag glyph already use) rather than a shadow
   *  variable, so the cycle always reflects reality even if the sort was
   *  last changed via the header menu rather than the toolbar. Clicking a
   *  DIFFERENT column than the one currently sorted resets the cycle to
   *  ascending on the new column. No-ops when no column resolves (empty
   *  sheet). */
  export function toolbarSort() {
    const col = selection.colSel ?? selection.anchor?.col ?? null;
    if (col === null || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (!column) return;
    if (viewState.sortColumnId !== column.id) {
      menuSort(col, 'asc');
    } else if (viewState.sortDirection === 'asc') {
      menuSort(col, 'desc');
    } else if (viewState.sortDirection === 'desc') {
      viewState = clearSort(viewState);
    } else {
      menuSort(col, 'asc');
    }
  }

  /** The toolbar button element the Filter popover was opened from —
   *  mirrors `stylePanelAnchorEl`'s “capture the real button, don't
   *  hardcode a position” approach (Part B) for the SAME reused
   *  `ShHeaderMenu` popover the column-header click door already renders,
   *  just from a different anchor element and pre-navigated to the filter
   *  view via `initialView`. */
  let toolbarFilterAnchorEl: HTMLElement | null = $state(null);
  /** The column `toolbarFilterAnchorEl`'s `ShHeaderMenu` instance is
   *  bound to — resolved once at open time (same “resolve on open, not
   *  reactively” convention `headerMenuAnchor`/`openHeaderMenu` use) so
   *  the popover doesn't silently retarget mid-interaction if the
   *  selection moves while it's open. */
  let toolbarFilterCol: number | null = $state(null);

  /** Toolbar Filter button (`Editor.svelte`'s `sheetGridRef.
   *  toolbarFilter()`) — resolves the target column the same forgiving
   *  way `toolbarSort` does, then opens `ShHeaderMenu` anchored to the
   *  toolbar's own Filter button (passed in as `anchorEl`), pre-navigated
   *  straight to the filter value-picker view (`initialView: 'filter'`).
   *  No-ops when no column resolves, OR when the resolved column doesn't
   *  support filtering (`filterSupported`) — same ruling-3 gate the
   *  header menu's own “Filter…” row enforces, reapplied here since this
   *  door skips that row entirely. Closes the column-header menu first if
   *  one happens to be open, so the two `ShHeaderMenu` instances never
   *  show at once (same “only one popover at a time” discipline
   *  `openStylePanel` applies against the context menu). */
  export function toolbarFilter(anchorEl: HTMLElement) {
    const col = selection.colSel ?? selection.anchor?.col ?? null;
    if (col === null || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (!column || !filterSupported(column.col_type)) return;
    closeHeaderMenu();
    toolbarFilterAnchorEl = anchorEl;
    toolbarFilterCol = col;
  }

  function closeToolbarFilter() {
    toolbarFilterAnchorEl = null;
    toolbarFilterCol = null;
  }

  /** #2339 PR 15b-beta: persists `ShOptionsPanel`'s new choices array via
   *  `updateSheetColumn({ optionsJson })` — same shape/error-handling as
   *  `menuTypeChange` right above it. `options_json`'s OTHER keys (there
   *  are none defined yet, per `docs/spec/sheets.md` §33.6 — `choices` is
   *  the only key any code in this repo reads or writes) are irrelevant
   *  here; this always writes a fresh `{"choices":[...]}` object rather
   *  than attempting to preserve unknown keys, matching
   *  `selectChoicesFromOptionsJson`'s own "choices is the only field"
   *  contract. */
  async function menuOptionsChange(col: number, choices: string[]) {
    const column = pivoted.columns[col];
    if (!column) return;
    try {
      await api.updateSheetColumn(column.id, { optionsJson: JSON.stringify({ choices }) });
      await refetchSheetData();
    } catch (e) {
      showToast(`Couldn't update options: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /** Delete-column flow (ruling 4: selection recovery clamps to the
   *  NEAREST remaining real column, never a ghost column, falling back to
   *  column 0 if none remain). Computed BEFORE the refetch (using the
   *  pre-delete column list/index), applied AFTER (against the post-
   *  delete, refetched `pivoted.columns`) — the target column's POSITION
   *  is what "nearest" means here, not its identity (it's gone). */
  async function menuDelete(col: number) {
    const column = pivoted.columns[col];
    if (!column) return;
    const wasSelectedCol = selection.anchor?.col === col || selection.colSel === col;
    try {
      await api.deleteSheetColumn(column.id);
      await refetchSheetData();
      if (viewState.sortColumnId === column.id) viewState = { ...viewState, sortColumnId: null, sortDirection: null };
      if (viewState.filterColumnId === column.id) viewState = { ...viewState, filterColumnId: null, filterValues: null };
      if (wasSelectedCol) {
        clampColumnAfterDelete(col);
      }
    } catch (e) {
      showToast(`Couldn't delete column: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /** Clamps the selection anchor to the nearest remaining REAL column
   *  after a delete, per ruling 4. `deletedCol` is the pre-delete index of
   *  the column that was just removed — the nearest survivor is simply
   *  `min(deletedCol, pivoted.columns.length - 1)` against the POST-delete
   *  column list (everything at or past the deleted index shifted left by
   *  one; clamping to the new last-index handles the "deleted the last
   *  column" case the same way). Falls back to column 0 (ruling 4: "if no
   *  columns remain, fall back to column 0") when the sheet is now
   *  columnless — `clampCoord` alone can't express that fallback since a
   *  0-column sheet still has ghost columns to clamp into, which ruling 4
   *  explicitly does NOT want here (recovery targets a REAL column). */
  function clampColumnAfterDelete(deletedCol: number) {
    const row = selection.anchor?.row ?? 0;
    if (pivoted.columns.length === 0) {
      selection = { anchor: { row, col: 0 }, range: null, colSel: null, rowSel: null };
      return;
    }
    const nearest = Math.min(deletedCol, pivoted.columns.length - 1);
    const clamped = clampCoord({ row, col: nearest }, bounds());
    selection = { anchor: clamped, range: null, colSel: null, rowSel: null };
  }

  function handleRowGutterClick(row: number) {
    dispatch({ type: 'SELECT_ROW', row });
  }

  // ── Checkbox toggle (ruling: no editor, direct-commit) ──────────────

  function maybeToggleCheckboxOnClick(row: number, col: number) {
    if (row >= rowOrder.length || col >= pivoted.columns.length) return;
    const column = pivoted.columns[col];
    if (column?.col_type === 'checkbox') {
      toggleCheckbox(row, col);
    }
  }

  async function toggleCheckbox(row: number, col: number) {
    const denseRow = underlyingRow(row);
    const cell = denseRow?.cells[col]?.cell ?? null;
    const next = toggleCheckboxValue(cell?.raw_value);
    try {
      await commitCellValue(row, col, next);
    } catch (e) {
      showToast(`Couldn't update cell: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  // ── Cell editor open/commit/cancel (text/number/currency/date) ─────

  function editorVariantFor(colType: string): 'text' | 'number' | 'currency' {
    return colType === 'number' ? 'number' : colType === 'currency' ? 'currency' : 'text';
  }

  function openEditorForCell(row: number, col: number, opts: { clearWithChar?: string; origin?: 'cell' | 'bar' }) {
    const column = pivoted.columns[col];
    if (!column) return;
    const denseRow = underlyingRow(row);
    const cell = denseRow?.cells[col]?.cell ?? null;
    const rawValue = cell?.raw_value ?? '';

    let initialValue: string;
    if (opts.clearWithChar != null) {
      initialValue = opts.clearWithChar;
    } else if (column.col_type === 'date') {
      initialValue = isoToDateMask(rawValue);
    } else {
      initialValue = rawValue;
    }

    editorError = null;
    // #2874 item 12 Part D: `origin` defaults to 'cell' for every existing
    // caller (dblclick, type-to-edit, ghost-row) — only `barBeginEdit`
    // passes 'bar' explicitly.
    editing = { row, col, colType: column.col_type, value: initialValue, isFormula: initialValue.startsWith('='), selectionStart: null, selectionEnd: null, origin: opts.origin ?? 'cell' };
  }

  function openSelectEditor(row: number, col: number) {
    const column = pivoted.columns[col];
    if (!column) return;
    const denseRow = underlyingRow(row);
    const cell = denseRow?.cells[col]?.cell ?? null;
    editorError = null;
    // Ruling 2: select never enters formula mode regardless of the
    // cell's stored value's shape. `origin: 'cell'` — `barBeginEdit` never
    // reaches this path (it no-ops for select anchors).
    editing = { row, col, colType: 'select', value: cell?.raw_value ?? '', isFormula: false, selectionStart: null, selectionEnd: null, origin: 'cell' };
  }

  function selectChoicesFor(column: SheetColumn | undefined): string[] {
    return selectChoicesFromOptionsJson(column?.options_json);
  }

  /** Reclaims DOM focus for the grid wrapper. Per the DOM spec, removing the
   *  currently-focused element (an editor's input/popover) from the document
   *  drops focus to <body>, not to any focusable ancestor — so each editor-
   *  closing path that unmounts a focused editor via a keyboard-driven cause
   *  must explicitly call this to keep the grid keyboard-navigable. NOT
   *  called on cause === 'blur': a blur means the user clicked elsewhere on
   *  purpose, and focus should stay wherever they clicked. */
  function refocusGrid() {
    gridEl?.focus();
  }

  /** Commits `editing`'s buffer and closes the editor immediately
   *  (optimistic — the common case is success). The network commit runs in
   *  the background; on rejection (invalid formula, network error) the
   *  editor is REOPENED at its original cell with the error shown and the
   *  unsent value preserved, per §3's "never silently lose an edit."
   *  Navigation (Enter/Tab) fires immediately too, matching standard
   *  spreadsheet responsiveness — it isn't blocked on the network
   *  round-trip. */
  function handleEditorCommit(cause: 'enter' | 'tab' | 'shift-tab' | 'blur') {
    if (!editing) return;
    const snapshot = editing;
    const commitValue = valueForCommit(snapshot.colType, snapshot.value);
    editing = null;
    editorError = null;

    if (cause === 'enter') dispatch({ type: 'ENTER' });
    else if (cause === 'tab') dispatch({ type: 'TAB', shift: false });
    else if (cause === 'shift-tab') dispatch({ type: 'TAB', shift: true });

    if (cause === 'enter' || cause === 'tab' || cause === 'shift-tab') refocusGrid();

    commitCellValue(snapshot.row, snapshot.col, commitValue).catch((e) => {
      editing = { ...snapshot };
      editorError = e instanceof Error ? e.message : String(e);
      dispatch({ type: 'SELECT_CELL', coord: { row: snapshot.row, col: snapshot.col } });
    });
  }

  function handleEditorCancel() {
    editing = null;
    editorError = null;
    refocusGrid();
  }

  function handleSelectCommit(choice: string) {
    if (!editing) return;
    const snapshot = editing;
    editing = null;
    editorError = null;
    refocusGrid();
    commitCellValue(snapshot.row, snapshot.col, choice).catch((e) => {
      showToast(`Couldn't update cell: ${e instanceof Error ? e.message : String(e)}`);
    });
  }

  function handleSelectCancel() {
    editing = null;
    editorError = null;
    refocusGrid();
  }

  // ── Header rename (#2339 PR 10, §4) ─────────────────────────────────
  // §33.3.5 amendment (item 44 / #2874): ghost-COLUMN typing no longer
  // calls this — the only remaining callers are `insertColumn()` (the
  // explicit toolbar/context-menu "add column" action, its own
  // birth-moment rename affordance, untouched by this amendment) and
  // `menuRename()` (right-click header menu → Rename), the sanctioned
  // rename path per §33.3.5.

  function startHeaderRename(col: number) {
    headerRename = { col, value: '' };
  }

  function commitHeaderRename() {
    if (!headerRename) return;
    const snapshot = headerRename;
    headerRename = null;
    const column = pivoted.columns[snapshot.col];
    if (!column) return;
    api.updateSheetColumn(column.id, { name: snapshot.value }).then((updated) => {
      if (!sheetData) return;
      const idx = sheetData.columns.findIndex((c) => c.id === column.id);
      if (idx < 0) return;
      const columns = [...sheetData.columns];
      columns[idx] = updated;
      sheetData = { ...sheetData, columns };
      pivoted = pivotSheetData(columns, sheetData.rows, sheetData.cells);
    }).catch((e) => {
      showToast(`Couldn't rename column: ${e instanceof Error ? e.message : String(e)}`);
    });
  }

  function cancelHeaderRename() {
    // The column was already created with name: "" — an empty name is a
    // valid, explicitly-designed state (README: "headers seed empty, not
    // 'Column 1'"), so Escape needs no API call, just closing the input.
    headerRename = null;
  }

  function handleHeaderRenameKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      commitHeaderRename();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancelHeaderRename();
    } else {
      e.stopPropagation();
    }
  }

  // ── Ghost typing-to-commit (#2339 PR 10, §4 — 4 cases) ──────────────

  function isPrintableKey(e: KeyboardEvent): boolean {
    return e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey;
  }

  function handleTypeToEdit(e: KeyboardEvent) {
    if (!selection.anchor) return;
    const { row, col } = selection.anchor;
    const isGhostRow = row >= rowOrder.length;
    const isGhostCol = col >= pivoted.columns.length;

    if (isGhostRow || isGhostCol) {
      e.preventDefault();
      // Item 23 / #2874: how many ghost rows/columns past the last real
      // one was the click? `row`/`col` are absolute grid coordinates;
      // subtracting the real-row/column count and adding 1 gives a
      // 1-based "create this many" count. Offset 1 = the first ghost
      // row/col (today's only-supported case) so this is backward
      // compatible. Non-ghost axes get offset 0 (nothing to create on
      // that axis).
      const rowOffset = isGhostRow ? row - rowOrder.length + 1 : 0;
      const colOffset = isGhostCol ? col - pivoted.columns.length + 1 : 0;
      beginGhostCommit(row, col, e.key, rowOffset, colOffset);
      return;
    }

    const column = pivoted.columns[col];
    if (!column) return;

    if (column.col_type === 'checkbox') {
      // No edit mode at all for checkbox (ruling) — only Space toggles;
      // any other printable key is a no-op (there's nothing sensible to
      // "type into" a checkbox).
      if (e.key === ' ') {
        e.preventDefault();
        toggleCheckbox(row, col);
      }
      return;
    }

    if (column.col_type === 'select') {
      // Select is a dropdown, not a combobox (design annotation) — there's
      // no typed buffer to seed a fixed choice list with, so type-to-edit
      // has no meaning here. Double-click is the only entry path (§2).
      return;
    }

    e.preventDefault();
    openEditorForCell(row, col, { clearWithChar: e.key });
  }

  /** `rowOffset`/`colOffset` (item 23 / #2874): 1-based count of
   *  rows/columns to create on the corresponding ghost axis (0 = that
   *  axis isn't ghost, nothing to create there). Each axis's batch runs
   *  as ONE backend transaction (one snapshot, one recompute+regen) via
   *  `createSheetRowsBatch`/`createSheetColumnsBatch` — both axes at
   *  once (Case 3) still runs as TWO transactions (rows and columns are
   *  independent structural axes, same reasoning the pre-existing
   *  `Promise.all` shape already relied on). The typed value always
   *  targets the LAST row/column in the returned batch — everything
   *  before it is an intentionally empty intermediate row/column. */
  async function beginGhostCommit(
    row: number,
    col: number,
    char: string,
    rowOffset: number,
    colOffset: number
  ) {
    const isGhostRow = rowOffset > 0;
    const isGhostCol = colOffset > 0;
    ghostTyping = { row, col, char };

    try {
      let rowId: string;
      let columnId: string;

      if (isGhostRow && !isGhostCol) {
        // Case 1: ghost row only — create `rowOffset` rows in one txn,
        // typed value lands in the LAST one.
        const newRows = await api.createSheetRowsBatch(noteId, rowOffset);
        rowId = newRows[newRows.length - 1].id;
        columnId = pivoted.columns[col].id;
      } else if (!isGhostRow && isGhostCol) {
        // Case 2: ghost column only — create `colOffset` columns in one
        // txn, typed value lands in the LAST one.
        const newCols = await api.createSheetColumnsBatch(noteId, colOffset);
        columnId = newCols[newCols.length - 1].id;
        rowId = underlyingRow(row)!.row.id;
      } else {
        // Case 3: both ghost — independent axes, order doesn't matter,
        // but each axis's batch is its own transaction (rows and columns
        // don't share a snapshot/regen — same independence the
        // pre-existing single-insert Promise.all already relied on).
        const [newRows, newCols] = await Promise.all([
          api.createSheetRowsBatch(noteId, rowOffset),
          api.createSheetColumnsBatch(noteId, colOffset),
        ]);
        rowId = newRows[newRows.length - 1].id;
        columnId = newCols[newCols.length - 1].id;
      }

      await api.updateSheetCell(rowId, columnId, char, null);
      await refetchSheetData();

      const underlyingRowIdx = pivoted.rows.findIndex((r) => r.row.id === rowId);
      const newColIdx = pivoted.columns.findIndex((c) => c.id === columnId);
      ghostTyping = null;
      if (underlyingRowIdx < 0 || newColIdx < 0) return;
      // Display-index conversion (#2339 PR 11): with no active sort/filter
      // this is the identity map (newRowIdx === underlyingRowIdx). With a
      // filter active, a freshly-created row can land outside the visible
      // set (e.g. its new cell doesn't match the active filter value) —
      // `displayRowIndexFor` returns -1 in that case, and selection simply
      // doesn't move onto a now-invisible row (documented edge case, not
      // exercised by ghost-commit's own tests, which run with no view
      // active — sort/filter and ghost-commit are orthogonal features).
      const newRowIdx = displayRowIndexFor(underlyingRowIdx);
      if (newRowIdx < 0) return;

      dispatch({ type: 'SELECT_CELL', coord: { row: newRowIdx, col: newColIdx } });

      // §33.3.5 amendment (item 44 / #2874, Matt ruling): typing into a
      // ghost column (cases 2/3) NEVER opens the header rename input —
      // that superseded PR 10's original "birth-moment" rename-on-create
      // behavior, which stole focus into an empty rename field mid-
      // keystroke and silently swallowed the rest of what the user typed.
      // All three cases now behave identically: the cell editor stays
      // open on the new cell with the typed character in it, exactly like
      // ghost-row-only (case 1). The new column keeps its empty name
      // (`name: ''`, set at creation) until the user renames it from the
      // header menu (right-click → Rename) — see `menuRename`/
      // `startHeaderRename`, which are UNCHANGED and still the only way
      // to enter rename mode. checkbox/select have no typed-buffer editor
      // to reopen (see handleTypeToEdit) — the typed character has
      // already been written verbatim to the cell by the update_sheet_cell
      // call above, so we just leave the new cell selected in that case
      // rather than trying to open a nonexistent editor. (In practice a
      // freshly-created ghost column always defaults to `text`, so this
      // branch is unreachable for isGhostCol today — kept for parity with
      // case 1 and to stay correct if that default ever changes.)
      const newColumn = pivoted.columns[newColIdx];
      if (newColumn && newColumn.col_type !== 'checkbox' && newColumn.col_type !== 'select') {
        openEditorForCell(newRowIdx, newColIdx, { clearWithChar: char });
      }
    } catch (e) {
      ghostTyping = null;
      const what = isGhostRow && isGhostCol ? 'row and column' : isGhostRow ? 'row' : 'column';
      showToast(`Couldn't create ${what}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  // ── Toolbar-facing imperative API (bind:this from Editor.svelte) ────

  /** Inserts a new row and selects its first cell. With no args (the
   *  default), appends at the end — same call as ghost-row-commit, minus
   *  the keystroke-triggered value write. Exposed for SheetToolbar's
   *  insert-row button (ruling 5), called with no args there — DO NOT
   *  pass reference/position from the toolbar.
   *
   *  §33.3.4 positional insert: pass `referenceRowId` + `position` to
   *  insert relative to an existing row instead of appending — used by
   *  the context menu's "Insert row above" (`position: 'before'`).
   *  Selection lands on the new row at its INSERTED position either way
   *  (append or positional), same first-cell-select UX. */
  export async function insertRow(referenceRowId?: string, position?: 'before' | 'after') {
    try {
      const newRow = await api.createSheetRow(noteId, referenceRowId, position);
      await refetchSheetData();
      const underlyingIdx = pivoted.rows.findIndex((r) => r.row.id === newRow.id);
      const idx = underlyingIdx >= 0 ? displayRowIndexFor(underlyingIdx) : -1;
      if (idx >= 0) dispatch({ type: 'SELECT_CELL', coord: { row: idx, col: 0 } });
    } catch (e) {
      showToast(`Couldn't add row: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /** Inserts a new (empty-named, text-typed) column and enters header-
   *  rename mode on it immediately, matching ghost-column-commit's own
   *  birth-moment rename affordance. With no args (the default), appends
   *  at the end. Exposed for SheetToolbar's insert-column button (ruling
   *  5), called with no args there — DO NOT pass reference/position from
   *  the toolbar.
   *
   *  §33.3.4 positional insert: pass `referenceColumnId` + `position` to
   *  insert relative to an existing column instead of appending — used
   *  by the column header menu's "Insert column left" row (`position:
   *  'before'`; #2874 item 31 moved this row from `ShCellContextMenu` to
   *  `ShHeaderMenu`). Header-rename mode opens on the new column at its
   *  INSERTED position either way (append or positional), same
   *  birth-moment UX. */
  export async function insertColumn(referenceColumnId?: string, position?: 'before' | 'after') {
    try {
      const newCol = await api.createSheetColumn(noteId, '', 'text', null, referenceColumnId, position);
      await refetchSheetData();
      const idx = pivoted.columns.findIndex((c) => c.id === newCol.id);
      if (idx >= 0) {
        dispatch({ type: 'SELECT_COLUMN', col: idx });
        startHeaderRename(idx);
      }
    } catch (e) {
      showToast(`Couldn't add column: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  // ── Keyboard nav ─────────────────────────────────────────────────────

  /** Resolves the currently-selected cell's DOM element, mirroring the
   *  toolbar's `styleBtnEl` anchor pattern so `openStylePanel` positions
   *  the panel under the selected cell instead of falling back to the
   *  hardcoded `{x:12,y:78}` sidebar position (#2874 item 21). Tries the
   *  real-cell testid first, then the ghost-cell testid, so a fresh sheet
   *  (all ghost cells, no real cells committed yet) still anchors to the
   *  actual selected cell's rect instead of falling back to the sidebar
   *  position. Returns `undefined` only when there's no anchor cell at
   *  all — the caller falls through to the existing fallback in that
   *  case, same as today. */
  function selectedCellEl(): HTMLElement | undefined {
    const a = selection.anchor;
    if (!a || !gridEl) return undefined;
    return (
      gridEl.querySelector<HTMLElement>(`[data-testid="sheet-cell-${a.row}-${a.col}"]`) ??
      gridEl.querySelector<HTMLElement>(`[data-testid="sheet-ghost-cell-${a.row}-${a.col}"]`) ??
      undefined
    );
  }

  function handleKeydown(e: KeyboardEvent) {
    // While a cell editor or the header-rename input is open, THEY own
    // keydown (their own listeners stopPropagation on every key) — this is
    // a defensive top-level guard as well, so e.g. the select popover
    // (which only intercepts Escape) never leaks stray keys into grid nav.
    // #2339 PR 11: an open `ShHeaderMenu` owns keydown the same way (its
    // own Escape/type-submenu-back handling) — added to the same guard.
    if (editing || headerRename || openHeaderMenu !== null) return;

    // #2339 PR 15d-beta — ⌘⇧S opens the style panel (Matt's ruling
    // 2026-09-05: a ruled ADDITION to the design's menu-only depiction of
    // the "Style cells…" entry point, same status as Delete/Backspace's
    // own keyboard-shortcut ruling elsewhere in this file). Checked BEFORE
    // the switch below (rather than as a `case 's'`/`case 'S'` branch of
    // it) so a plain, unmodified 's' keystroke still falls through to the
    // `default`/`isPrintableKey` type-to-overwrite path unchanged — only
    // the exact ⌘⇧S chord is intercepted here.
    if (e.metaKey && e.shiftKey && (e.key === 's' || e.key === 'S')) {
      e.preventDefault();
      openStylePanel(selectedCellEl());
      return;
    }

    const jump = e.metaKey || e.ctrlKey;
    switch (e.key) {
      case 'ArrowUp':
        e.preventDefault();
        dispatch({ type: 'MOVE', direction: 'up', extend: e.shiftKey, jumpToEdge: jump });
        break;
      case 'ArrowDown':
        e.preventDefault();
        dispatch({ type: 'MOVE', direction: 'down', extend: e.shiftKey, jumpToEdge: jump });
        break;
      case 'ArrowLeft':
        e.preventDefault();
        dispatch({ type: 'MOVE', direction: 'left', extend: e.shiftKey, jumpToEdge: jump });
        break;
      case 'ArrowRight':
        e.preventDefault();
        dispatch({ type: 'MOVE', direction: 'right', extend: e.shiftKey, jumpToEdge: jump });
        break;
      case 'Tab':
        e.preventDefault();
        dispatch({ type: 'TAB', shift: e.shiftKey });
        break;
      case 'Enter':
        // Ruling 1: Enter is navigation ONLY — it never opens an editor,
        // even on an already-anchored cell.
        e.preventDefault();
        dispatch({ type: 'ENTER' });
        break;
      case 'Escape':
        e.preventDefault();
        dispatch({ type: 'ESCAPE' });
        break;
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        clearSelectedContents();
        break;
      default:
        if (isPrintableKey(e)) {
          handleTypeToEdit(e);
        }
        break;
    }
  }

  function gridTemplateColumns(): string {
    const widths = pivoted.columns.map((c) => `${c.effectiveWidth}px`);
    const ghostWidths = Array(GHOST_COLS).fill(`${GHOST_COLUMN_WIDTH}px`);
    return [`${GUTTER_W}px`, ...widths, ...ghostWidths].join(' ');
  }

  function colRef(i: number): string {
    let n = i;
    let ref = '';
    do {
      ref = String.fromCharCode(65 + (n % 26)) + ref;
      n = Math.floor(n / 26) - 1;
    } while (n >= 0);
    return ref;
  }

  // typeGlyph() moved to sheetCellRender.ts (#2339 PR 15b-beta) — imported
  // above — so ShSnapshotPreview.svelte's read-only version-history
  // preview grid can share the exact same glyph mapping without
  // duplicating this switch statement. Zero behavior change here.
</script>

<!-- #2339 PR 12: new outer flex-column wrapper so `SheetViewStrip` sits
     ABOVE the scrollable grid area (not inside it — `.sh-gridwrap` itself
     is the `overflow: auto` scroll container, and the strip must stay put
     while grid content scrolls under it, matching the design's flex-
     column frame between toolbar and grid). `.sh-gridwrap` below is
     otherwise unchanged apart from #2874 item 19's `onmouseup` removal
     (see `handleCellMouseDown`/`handleDragSelectMouseUp` above) — same
     testid, same bindings, same keydown handler. -->
<div class="sh-gridcol">
  {#if !loading && !loadError}
    <SheetViewStrip
      {sortColumn}
      sortDirection={viewState.sortDirection}
      {filterColumn}
      filterValues={viewState.filterValues}
      onClearSort={handleClearSort}
      onClearFilter={handleClearFilter}
    />
  {/if}
  <div class="sh-gridwrap" data-testid="sheet-grid-wrap" bind:this={gridEl} onkeydown={handleKeydown} tabindex="0" role="grid" aria-label="Sheet grid">
  {#if loading}
    <div class="sh-loading">Loading sheet…</div>
  {:else if loadError}
    <div class="sh-error-banner">Failed to load sheet: {loadError}</div>
  {:else}
    <div class="sh-grid" style:grid-template-columns={gridTemplateColumns()} data-testid="sheet-grid">
      <!-- corner -->
      <div class="sh-corner" style:height="{HEAD_H}px"></div>

      <!-- real column headers -->
      {#each pivoted.columns as column, ci (column.id)}
        <div
          class="sh-h"
          class:is-sel={selection.colSel === ci}
          class:is-open={openHeaderMenu === ci}
          style:height="{HEAD_H}px"
          style:position="relative"
          role="columnheader"
          tabindex="-1"
          onclick={() => handleColumnHeaderClick(ci)}
          oncontextmenu={(e) => handleColumnHeaderContextMenu(ci, e)}
          data-testid="sheet-col-header-{ci}"
        >
          <span class="sh-h__ref">{colRef(ci)}</span>
          <span class="sh-h__type">{typeGlyph(column.col_type)}</span>
          {#if headerRename && headerRename.col === ci}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="sh-h__ren"
              autofocus
              value={headerRename.value}
              oninput={(e) => { if (headerRename) headerRename.value = (e.target as HTMLInputElement).value; }}
              onkeydown={handleHeaderRenameKeydown}
              onblur={commitHeaderRename}
              onclick={(e) => e.stopPropagation()}
              data-testid="sheet-header-rename-input"
            />
          {:else}
            <span class="sh-h__name">{column.name}</span>
          {/if}
          {#if column.col_type === 'currency'}
            <span class="sh-h__code">{currencyCode(column.options_json)}</span>
          {/if}
          {#if viewState.sortColumnId === column.id}
            <span class="sh-h__flag" data-testid="sheet-header-flag-sort-{ci}">{viewState.sortDirection === 'asc' ? '↑' : '↓'}</span>
          {/if}
          {#if viewState.filterColumnId === column.id}
            <span class="sh-h__flag" data-testid="sheet-header-flag-filter-{ci}">▽</span>
          {/if}
          {#if openHeaderMenu === ci}
            <ShHeaderMenu
              column={pivoted.columns[ci]}
              anchorEl={headerMenuAnchor}
              filterValues={viewState.filterColumnId === column.id ? viewState.filterValues : null}
              onRename={() => menuRename(ci)}
              onTypeChange={(colType) => menuTypeChange(ci, colType)}
              onInsertColumn={() => { insertColumn(pivoted.columns[ci].id, 'before'); closeHeaderMenu(); }}
              onSortAsc={() => menuSort(ci, 'asc')}
              onSortDesc={() => menuSort(ci, 'desc')}
              onFilterChange={(values) => menuFilterChange(ci, values)}
              onDelete={() => menuDelete(ci)}
              onClose={closeHeaderMenu}
              onOptionsChange={(choices) => menuOptionsChange(ci, choices)}
            />
          {/if}
          <span
            class="sh-h__grip"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize column {colRef(ci)}"
            onmousedown={(e) => startColumnResize(ci, e)}
            onclick={(e) => e.stopPropagation()}
            oncontextmenu={(e) => e.stopPropagation()}
            data-testid="sheet-col-grip-{ci}"
          ></span>
        </div>
      {/each}

      <!-- ghost column headers -->
      {#each Array(GHOST_COLS) as _, gi}
        <div class="sh-h sh-h--ghost" style:height="{HEAD_H}px" data-testid="sheet-ghost-col-header-{gi}">
          <span class="sh-h__ref">{colRef(pivoted.columns.length + gi)}</span>
        </div>
      {/each}

      <!-- real rows -->
      {#each visibleRows as denseRow, ri (denseRow.row.id)}
        <div
          class="sh-g"
          class:is-sel={selection.rowSel === ri}
          style:height="{denseRow.height}px"
          style:position="relative"
          role="rowheader"
          tabindex="-1"
          onclick={() => handleRowGutterClick(ri)}
          data-testid="sheet-row-gutter-{ri}"
        >
          {rowOrder[ri] + 1}
          <span
            class="sh-g__grip"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize row {rowOrder[ri] + 1}"
            onmousedown={(e) => startRowResize(ri, e)}
            onclick={(e) => e.stopPropagation()}
            data-testid="sheet-row-grip-{ri}"
          ></span>
        </div>
        {#each pivoted.columns as column, ci (column.id)}
          {@const dc = denseRow.cells[ci]}
          {@const choices = column.col_type === 'select' ? selectChoicesFor(pivoted.columns[ci]) : undefined}
          {@const kind = cellKind(dc?.cell ?? null, column.col_type, choices)}
          {@const isEditing = editing !== null && editing.row === ri && editing.col === ci}
          {@const isMismatchTooltipOpen = kind === 'mis' && mismatchTooltip !== null && mismatchTooltip.row === ri && mismatchTooltip.col === ci}
          {@const isErrorPopoverOpen = kind === 'err' && selection.anchor !== null && selection.anchor.row === ri && selection.anchor.col === ci}
          {@const refEdges = cellRefEdges(ri, ci)}
          <div
            class={cellClass(ri, ci, dc?.cell ?? null, column.col_type, selection, choices)}
            class:is-ref={refEdges !== null}
            class:is-refedge-t={!!refEdges?.top}
            class:is-refedge-b={!!refEdges?.bottom}
            class:is-refedge-l={!!refEdges?.left}
            class:is-refedge-r={!!refEdges?.right}
            style:height="{denseRow.height}px"
            style={cellInlineStyle(dc?.cell?.style_json ?? null, kind)}
            role="gridcell"
            tabindex="-1"
            onclick={(e) => handleCellClick(ri, ci, e)}
            onmousedown={(e) => handleCellMouseDown(ri, ci, e)}
            onmouseenter={() => { handleCellMouseEnter(ri, ci); if (kind === 'mis') mismatchTooltip = { row: ri, col: ci }; }}
            onmouseleave={() => { if (mismatchTooltip && mismatchTooltip.row === ri && mismatchTooltip.col === ci) mismatchTooltip = null; }}
            ondblclick={() => handleCellDblClick(ri, ci)}
            oncontextmenu={(e) => handleCellContextMenu(ri, ci, e)}
            data-testid="sheet-cell-{ri}-{ci}"
          >
            {#if isEditing && editing && editing.colType === 'select'}
              <SheetCellEditorSelect
                value={editing.value}
                choices={selectChoicesFor(pivoted.columns[ci])}
                onCommit={handleSelectCommit}
                onCancel={handleSelectCancel}
                onOptionsChange={(choices) => menuOptionsChange(ci, choices)}
              />
            {:else if isEditing && editing && editing.isFormula}
              <!-- #2339 PR 14: formula-editing mode. `isFormula` is decided
                   ONCE at editor-open time (see the `EditingCell` doc
                   comment) from whichever buffer the session opened with
                   — checked BEFORE the date/text branches so any
                   non-checkbox/non-select column can enter it (ruling 2
                   excludes checkbox/select entirely: checkbox has no
                   editor at all, select's branch above always wins for
                   that column type regardless of the buffer). -->
              <SheetCellEditorFormula
                bind:value={editing.value}
                selectionStart={editing.selectionStart}
                selectionEnd={editing.selectionEnd}
                onCommit={handleEditorCommit}
                onCancel={handleEditorCancel}
                onSelectionChange={setEditingSelection}
                origin={editing.origin}
              />
            {:else if isEditing && editing && editing.colType === 'date'}
              <SheetCellEditorDate bind:value={editing.value} onCommit={handleEditorCommit} onCancel={handleEditorCancel} origin={editing.origin} />
            {:else if isEditing && editing}
              <SheetCellEditorText
                bind:value={editing.value}
                variant={editorVariantFor(editing.colType)}
                currencyCode={currencyCode(column.options_json)}
                onCommit={handleEditorCommit}
                onCancel={handleEditorCancel}
                origin={editing.origin}
              />
            {:else if column.col_type === 'checkbox'}
              <!-- #2874 item 39 (Matt's ruling 2026-09-15): the checked
                   state renders an inline SVG check mark inside the box —
                   `stroke="currentColor"` inherits `.sh-box`'s existing
                   `color: transparent` (unchecked) / `.sh-box.is-on {
                   color: var(--accent) }` (checked) rules, so the glyph
                   uses those pre-existing (previously dead) color
                   declarations verbatim. No new color values introduced. -->
              {@const checked = !!dc?.cell?.raw_value && dc.cell.raw_value !== '0'}
              <span class="sh-box" class:is-on={checked}>
                {#if checked}<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>{/if}
              </span>
            {:else if column.col_type === 'select' && dc?.cell?.raw_value && kind !== 'mis'}
              <span class="sh-pill">{dc.cell.raw_value}</span>
            {:else if column.col_type === 'currency' && dc?.cell && kind !== 'mis'}
              <!-- #2874 item 38 (Matt's ruling 2026-09-15): currency
                   cells render via the SHARED `formatCurrencyValue()`
                   (sheetCellRender.ts) — symbol + two fixed decimals
                   (e.g. `$930.00`), not the raw decimal value followed by
                   a separate `.sh-cur` ISO-code span. The `.sh-cur` span
                   is removed from CELLS ONLY (ruling 2) — the column
                   header's `.sh-h__code` label above is untouched. -->
              <span class="sh-c__v">{formatCurrencyValue(displayValue(dc.cell), column.options_json)}</span>
            {:else}
              <span class="sh-c__v">{displayValue(dc?.cell ?? null)}</span>
            {/if}
            {#if isEditing && editorError}
              <span class="sh-ed__err">{editorError}</span>
            {/if}
            {#if kind === 'err'}<span class="sh-w sh-w--err"></span>{/if}
            {#if kind === 'mis'}<span class="sh-w sh-w--mis" data-testid="sheet-mismatch-wedge-{ri}-{ci}"></span>{/if}
            {#if isMismatchTooltipOpen}
              <div class="sh-mis-tip" role="tooltip" data-testid="sheet-mismatch-tooltip">
                <div class="sh-mis-tip__t">⚠ {mismatchExplanation(column.col_type)}</div>
                <div class="sh-mis-tip__sub">Kept as text, exactly as typed.</div>
              </div>
            {/if}
            {#if isErrorPopoverOpen}
              <ShErrorPopover errorCode={dc?.cell?.computed_value ?? ''} rawFormula={dc?.cell?.raw_value ?? ''} />
            {/if}
          </div>
        {/each}
        <!-- ghost cells for this real row -->
        {#each Array(GHOST_COLS) as _, gi}
          {@const gCol = pivoted.columns.length + gi}
          {@const isTyping = ghostTyping !== null && ghostTyping.row === ri && ghostTyping.col === gCol}
          <div
            class="sh-c sh-c--ghost"
            class:is-typing={isTyping}
            style:height="{denseRow.height}px"
            role="gridcell"
            tabindex="-1"
            onmousedown={(e) => handleCellMouseDown(ri, gCol, e)}
            onmouseenter={() => handleCellMouseEnter(ri, gCol)}
            oncontextmenu={(e) => handleCellContextMenu(ri, gCol, e)}
            class:is-anchor={isAnchor({ row: ri, col: gCol }, selection.anchor)}
            class:is-range={isInRange({ row: ri, col: gCol }, selection.range)}
            data-testid="sheet-ghost-cell-{ri}-{gi}"
          >
            {#if isTyping}<span class="sh-c__v">{ghostTyping!.char}</span>{/if}
          </div>
        {/each}
      {/each}

      <!-- ghost rows -->
      {#each Array(GHOST_ROWS) as _, gri}
        {@const gRow = rowOrder.length + gri}
        <div class="sh-g sh-g--ghost" style:height="{DEFAULT_ROW_HEIGHT}px" data-testid="sheet-ghost-row-gutter-{gri}">
          {gRow + 1}
        </div>
        {#each pivoted.columns as column, ci (column.id)}
          {@const isTyping = ghostTyping !== null && ghostTyping.row === gRow && ghostTyping.col === ci}
          <div
            class="sh-c sh-c--ghost"
            class:is-typing={isTyping}
            style:height="{DEFAULT_ROW_HEIGHT}px"
            role="gridcell"
            tabindex="-1"
            onmousedown={(e) => handleCellMouseDown(gRow, ci, e)}
            onmouseenter={() => handleCellMouseEnter(gRow, ci)}
            oncontextmenu={(e) => handleCellContextMenu(gRow, ci, e)}
            class:is-anchor={isAnchor({ row: gRow, col: ci }, selection.anchor)}
            class:is-range={isInRange({ row: gRow, col: ci }, selection.range)}
            data-testid="sheet-ghost-cell-{gRow}-{ci}"
          >
            {#if isTyping}<span class="sh-c__v">{ghostTyping!.char}</span>{/if}
          </div>
        {/each}
        {#each Array(GHOST_COLS) as _, gci}
          {@const gCol = pivoted.columns.length + gci}
          {@const isTyping = ghostTyping !== null && ghostTyping.row === gRow && ghostTyping.col === gCol}
          <div
            class="sh-c sh-c--ghost"
            class:is-typing={isTyping}
            style:height="{DEFAULT_ROW_HEIGHT}px"
            role="gridcell"
            tabindex="-1"
            onmousedown={(e) => handleCellMouseDown(gRow, gCol, e)}
            onmouseenter={() => handleCellMouseEnter(gRow, gCol)}
            oncontextmenu={(e) => handleCellContextMenu(gRow, gCol, e)}
            class:is-anchor={isAnchor({ row: gRow, col: gCol }, selection.anchor)}
            class:is-range={isInRange({ row: gRow, col: gCol }, selection.range)}
            data-testid="sheet-ghost-cell-{gRow}-{gCol}"
          >
            {#if isTyping}<span class="sh-c__v">{ghostTyping!.char}</span>{/if}
          </div>
        {/each}
      {/each}
    </div>
  {/if}

  {#if resizeState}
    <!-- Drag-time full-length boundary guide (#2339 PR 13) — absolutely
         positioned within `.sh-gridwrap` (its containing block, per the
         design's own markup, scope §7.2). Column drag: vertical line
         spanning the full grid height. Row drag: horizontal line spanning
         the full grid width. Neither affects layout — pure overlay. -->
    {#if resizeState.axis === 'col'}
      <div class="sh-guide" style:left="{resizeGuidePos}px" style:top="0" style:bottom="0" data-testid="sheet-resize-guide"></div>
    {:else}
      <div class="sh-guide sh-guide--row" style:top="{resizeGuidePos}px" style:left="0" style:right="0" data-testid="sheet-resize-guide"></div>
    {/if}
    <!-- Mono pixel readout pinned near the cursor (#2339 PR 13) —
         `position: fixed` so it escapes `.sh-gridwrap`'s own `overflow:
         auto` without needing scroll-offset math (scope §4.1/§7.3). -->
    <div
      class="sh-readout"
      style:position="fixed"
      style:left="{resizeReadoutPos.left}px"
      style:top="{resizeReadoutPos.top}px"
      data-testid="sheet-resize-readout"
    >{formatReadout(resizeState.currentSize)}</div>
  {/if}

  {#if toastMsg}
    <div class="sh-toast" role="alert" data-testid="sheet-toast">
      <span class="sh-toast__msg">{toastMsg}</span>
      <button class="sh-toast__dismiss" onclick={dismissToast} aria-label="Dismiss">✕</button>
    </div>
  {/if}

  {#if contextMenuCell && contextMenuPos}
    {#if contextMenuIsGhost}
      <!-- #2874 item 22 — ShGhostCellContextMenu, opened by right-click on
           a ghost row/column/both. Reduced 2-item menu: ghost cells have
           no contents to style/clear/export/delete. Both callbacks call
           `insertRow()`/`insertColumn()` with ZERO arguments (append via
           the MAX(sort_order) path) rather than resolving a reference id
           + position — passing `(undefined, 'before')` for a ghost
           coordinate hits a Rust backend guard that rejects mismatched
           reference/position pairs (see the item 22 scope doc §3). -->
      <ShGhostCellContextMenu
        x={contextMenuPos.x}
        y={contextMenuPos.y}
        onInsertRow={() => { insertRow(); closeContextMenu(); }}
        onInsertColumn={() => { insertColumn(); closeContextMenu(); }}
        onClose={closeContextMenu}
      />
    {:else}
      <!-- #2339 PR 15b-alpha (alpha) — ShCellContextMenu, opened by
           right-click on a real cell (`handleCellContextMenu`). #2339
           PR 15d-beta: "Style cells…" now opens `ShStylePanel` (the
           SECOND of the panel's "two doors") anchored at the same click
           point the context menu itself used, instead of alpha's
           close-only stub. #2874 item 11: Export sheet as CSV… row
           removed from this menu entirely — `ShSheetMenu.svelte` keeps
           its own independent Export CSV row (`sheet-more-menu-export`),
           zero shared code path with this component. #2874 item 31:
           "Insert column left" removed from this menu entirely — it now
           lives in `ShHeaderMenu`'s column-operations group, wired via
           the column-header instance below (`onInsertColumn` there).

           Positional insert (§33.3.4, item 3): `onInsertRow` still calls
           this component's OWN already-exported `insertRow` method (no
           new prop needed) — but now resolves `contextMenuCell` to the
           underlying row ID first (`underlyingRow`, the same lookup
           pattern every other cell handler in this file uses) and passes
           `(id, 'before')` so the new row lands immediately above the
           right-clicked cell instead of appending at the end. Only
           'before' is wired — 'after' exists in the IPC contract but has
           no UI entry point. -->
      <ShCellContextMenu
        x={contextMenuPos.x}
        y={contextMenuPos.y}
        cellCount={selectionCellCount}
        onStyleCells={() => { const pos = contextMenuPos; closeContextMenu(); openStylePanel(pos ?? undefined); }}
        onClearStyling={() => { clearSelectedStyling(); closeContextMenu(); }}
        onInsertRow={() => {
          const referenceRowId = contextMenuCell ? underlyingRow(contextMenuCell.row)?.row.id : undefined;
          insertRow(referenceRowId, 'before');
          closeContextMenu();
        }}
        onDeleteContents={() => { clearSelectedContents(); closeContextMenu(); }}
        onClose={closeContextMenu}
      />
    {/if}
  {/if}

  {#if stylePanelOpen}
    <!-- #2339 PR 15d-beta — ShStylePanel, the fill/text/border picker.
         `currentBg`/`currentFg`/`currentBorder` are the unanimity-derived
         props (Matt's ruling 4) computed above; each `onApply*` callback
         funnels into the single `applyStyleToSelection` handler, which
         resolves Matt's outer-edge border ruling (§3) per cell against
         the active selection's actual range shape. `onClearStyling` reuses
         the EXISTING `clearSelectedStyling` (alpha, also the context
         menu's own "Clear styling" row) rather than a second code path —
         "clear" always means the same thing regardless of which UI
         surface triggers it. -->
    <ShStylePanel
      x={stylePanelPos.x}
      y={stylePanelPos.y}
      anchorEl={stylePanelAnchorEl}
      currentBg={panelBg}
      currentFg={panelFg}
      currentBorder={panelBorder}
      onApplyFill={(color) => applyStyleToSelection({ bg: color === 'transparent' ? null : color })}
      onApplyInk={(color) => applyStyleToSelection({ fg: color })}
      onApplyBorder={(spec) => applyStyleToSelection({ border: spec })}
      onApplyBorderSide={(spec) => applyStyleToSelection({ borderSide: spec })}
      onApplyBorderWeight={(spec) => applyStyleToSelection({ borderWeight: spec })}
      onApplyBorderDashed={(spec) => applyStyleToSelection({ borderDashed: spec })}
      onClearStyling={clearSelectedStyling}
      onClose={closeStylePanel}
    />
  {/if}

  {#if toolbarFilterAnchorEl && toolbarFilterCol !== null && pivoted.columns[toolbarFilterCol]}
    <!-- #2874 item 16 Part C — toolbar Filter button's door into the
         SAME `ShHeaderMenu` the column-header click door renders above,
         just anchored to the toolbar's Filter button instead of a column
         header cell, and pre-navigated to the filter view
         (`initialView: 'filter'`) since there's no root menu to land on
         from this door. The wrapper below is `position: fixed` at the
         anchor button's own `getBoundingClientRect()` (mirrors Part B's
         `openStylePanel` anchor math) so `ShHeaderMenu`'s OWN `.sh-pop`
         positioning (`top: 100%; left: 0`, relative to this wrapper) drops
         it directly under the button regardless of where in the DOM tree
         this component happens to render — the toolbar lives in a sibling
         component (`Editor.svelte`'s `SheetToolbar`), so there is no
         shared positioned ancestor to rely on the way the column-header
         loop's own `position: relative` header cells provide. -->
    <div class="sh-toolbar-filter-anchor" style:top="{toolbarFilterAnchorEl.getBoundingClientRect().bottom}px" style:left="{toolbarFilterAnchorEl.getBoundingClientRect().left}px">
      <ShHeaderMenu
        column={pivoted.columns[toolbarFilterCol]}
        anchorEl={toolbarFilterAnchorEl}
        initialView="filter"
        filterValues={viewState.filterColumnId === pivoted.columns[toolbarFilterCol].id ? viewState.filterValues : null}
        onRename={() => {}}
        onTypeChange={() => {}}
        onInsertColumn={() => {}}
        onSortAsc={() => {}}
        onSortDesc={() => {}}
        onFilterChange={(values) => { if (toolbarFilterCol !== null) menuFilterChange(toolbarFilterCol, values); }}
        onDelete={() => {}}
        onClose={closeToolbarFilter}
        onOptionsChange={() => {}}
      />
    </div>
  {/if}
  </div>
</div>

<style>
  /* #2874 item 16 Part C — fixed-position anchor wrapper for the toolbar
     Filter button's `ShHeaderMenu` instance (see the `{#if
     toolbarFilterAnchorEl}` block above). Zero-size, positioned at the
     button's own viewport rect via inline `top`/`left`; `ShHeaderMenu`'s
     own `.sh-pop` (`position: absolute; top: 100%; left: 0`) then drops
     directly under it, same effective geometry as a column header cell's
     `position: relative` wrapper. */
  .sh-toolbar-filter-anchor { position: fixed; width: 0; height: 0; z-index: 30; }

  /* #2339 PR 12: the new outer wrapper — flex column so `SheetViewStrip`
     (flex-shrink: 0, same convention as `.sh-tb`) stacks above the
     scrollable `.sh-gridwrap`, which keeps its own `flex: 1`. */
  .sh-gridcol { display: flex; flex-direction: column; flex: 1; min-height: 0; }

  /* Ported verbatim (class names + rules) from docs/design/sheets/sheets.css
     — the grid + selection + ghost + corner-marker sections, plus PR 10's
     editor overlay/raw/rename/toast additions. */
  .sh-gridwrap { flex: 1; min-height: 0; overflow: auto; position: relative; background: var(--bg); outline: none; }
  .sh-grid { display: grid; width: max-content; font-size: 13px; position: relative; }
  .sh-c { --sh-c-border-shadow: 0 0 0 0 transparent; border-right: 1px solid var(--border-faint); border-bottom: 1px solid var(--border-faint); padding: 0 9px; display: flex; align-items: center; min-width: 0; position: relative; color: var(--fg-1); background: var(--bg); box-sizing: border-box; cursor: pointer; box-shadow: var(--sh-c-border-shadow); }
  .sh-c__v { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }

  .sh-h { position: sticky; top: 0; z-index: 3; background: var(--surface-1); border-right: 1px solid var(--border); border-bottom: 1px solid var(--border); display: flex; align-items: center; gap: 7px; padding: 0 8px; box-sizing: border-box; cursor: pointer; transition: background var(--dur-fast) var(--ease); }
  .sh-h:hover { background: var(--surface-2); }
  .sh-h__ref { font-family: var(--font-mono); font-size: 10px; color: var(--fg-4); flex-shrink: 0; letter-spacing: 0.02em; }
  .sh-h__type { width: 15px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: var(--fg-3); font-family: var(--font-mono); font-size: 11px; font-weight: 500; }
  .sh-h__name { flex: 1; min-width: 0; font-size: 12.5px; font-weight: 600; color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sh-h__code { font-family: var(--font-mono); font-size: 9.5px; color: var(--fg-4); flex-shrink: 0; letter-spacing: 0.04em; }
  .sh-h.is-sel { background: var(--accent-soft); }
  .sh-h.is-sel .sh-h__name { color: var(--accent); }
  /* #2339 PR 11 — ShHeaderMenu open state + sort/filter glyph, ported
     verbatim from docs/design/sheets/sheets.css. */
  .sh-h.is-open { background: var(--surface-2); box-shadow: inset 0 0 0 1px var(--accent); }
  .sh-h__flag { flex-shrink: 0; display: flex; color: var(--accent); font-size: 11px; }

  /* #2339 PR 13 — Column & Row Resize (ShResize), §33.3.3. All 7 classes
     ported byte-for-byte from docs/design/sheets/sheets.css L129–150
     (verbatim per the task brief — no rule text changed). Zero new design
     tokens; every var() used here already exists in src/app.css. */
  /* column resize hit zone — 6px straddling the header's right edge */
  .sh-h__grip{position:absolute;top:0;right:-3px;width:6px;height:100%;cursor:col-resize;z-index:4}
  .sh-h__grip:hover::after,.sh-h__grip.is-forced::after{content:'';position:absolute;top:0;right:2px;width:2px;height:100%;background:var(--accent)}
  /* row resize hit zone — bottom edge of the gutter cell only, so it can
     never be confused with a cell-content drag */
  .sh-g__grip{position:absolute;left:0;bottom:-3px;width:100%;height:6px;cursor:row-resize;z-index:4}
  .sh-g__grip:hover::after,.sh-g__grip.is-forced::after{content:'';position:absolute;left:0;bottom:2px;height:2px;width:100%;background:var(--accent)}
  /* live resize guide + readout */
  .sh-guide{position:absolute;top:0;bottom:0;width:2px;background:var(--accent);z-index:8;pointer-events:none}
  .sh-guide--row{left:0;right:0;top:auto;bottom:auto;width:auto;height:2px}
  .sh-readout{position:absolute;z-index:9;background:var(--surface-3);border:1px solid var(--border-strong);border-radius:var(--radius-sm);padding:3px 8px;font-family:var(--font-mono);font-size:11px;color:var(--fg-1);box-shadow:var(--shadow-pop);pointer-events:none;font-variant-numeric:tabular-nums}

  /* PR 10 §4 — inline birth-moment header rename input. Not in the design
     CSS verbatim (no dedicated class there); styled to sit flush in the
     `.sh-h__name` slot it replaces, keeping the cyan-ring editing language
     consistent with `.sh-ed`. */
  .sh-h__ren { flex: 1; min-width: 0; background: var(--surface-1); box-shadow: inset 0 0 0 1.5px var(--accent); border: 0; border-radius: 2px; outline: none; padding: 2px 5px; color: var(--fg-1); font-family: inherit; font-size: 12.5px; font-weight: 600; }

  .sh-corner { position: sticky; top: 0; left: 0; z-index: 5; background: var(--surface-1); border-right: 1px solid var(--border); border-bottom: 1px solid var(--border); box-sizing: border-box; }
  .sh-g { position: sticky; left: 0; z-index: 2; background: var(--surface-1); border-right: 1px solid var(--border); border-bottom: 1px solid var(--border-faint); box-sizing: border-box; display: flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-size: 10.5px; color: var(--fg-4); font-variant-numeric: tabular-nums; cursor: pointer; }
  .sh-g:hover { background: var(--surface-2); color: var(--fg-2); }
  .sh-g.is-sel { background: var(--accent-soft); color: var(--accent); }

  .sh-c--num, .sh-c--currency { justify-content: flex-end; font-variant-numeric: tabular-nums; }
  .sh-c--date { font-variant-numeric: tabular-nums; color: var(--fg-2); }
  .sh-c--check { justify-content: center; }
  .sh-box { width: 16px; height: 16px; border-radius: 4px; border: 1.5px solid var(--fg-4); display: flex; align-items: center; justify-content: center; color: transparent; }
  .sh-box.is-on { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); }
  .sh-pill { display: inline-flex; align-items: center; max-width: 100%; padding: 2px 8px; border-radius: var(--radius-sm); background: var(--surface-2); border: 1px solid var(--border); font-size: 11.5px; font-weight: 500; color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* #2874 item 38: `.sh-cur` (the ISO-code span appended after a currency
     cell's value, e.g. "USD") removed — currency cells now render a
     single symbol-prefixed, two-decimal string (`$930.00`) via the
     shared `formatCurrencyValue()` formatter. The header's `.sh-h__code`
     label (below) is unaffected — removal is cells-only (ruling 2). */

  .sh-w { position: absolute; width: 0; height: 0; pointer-events: none; }
  /* #2874 item 41: the cyan computed wedge (`.sh-w--calc`) was removed from
     the live grid — the status bar's formula count and the formula bar's
     fx indicator already communicate "this cell is a formula", and the
     wedge added no information the amber mismatch / red error wedges
     didn't already cover for the cases that actually need a warning.
     `ShSnapshotPreview.svelte` KEEPS its own equivalent wedge, since the
     snapshot preview has no formula bar and needs its own indicator. */
  .sh-w--err { left: 0; bottom: 0; border-left: 5px solid var(--status-danger); border-top: 5px solid transparent; }
  /* #2874 item 55: the formula-cell tint (`.sh-c--calc` background, both
     the base 0.035-alpha rule that lived here and the stronger
     0.18-alpha `.sh-c.is-range.sh-c--calc` rule below) was removed per
     Matt's ruling — no formula indicators anywhere in the grid; users
     add borders/backgrounds themselves if they want them. The
     `sh-c--calc` class itself is gone too (`sheetCellRender.ts`'s
     `cellClass()` no longer emits it — it had no other consumer).
     Removing this background declaration cannot regress a user-set
     background on a formula cell: `cellInlineStyle()` always emits a
     user `bg` as an inline `style="background:..."` attribute, which
     already won over this class rule by CSS precedence (inline beats
     class selector, neither side used `!important`) before this
     removal — so that path is unaffected either way. */
  .sh-c--err { background: rgba(239, 68, 68, 0.09); color: var(--status-danger); font-family: var(--font-mono); font-size: 11.5px; font-weight: 500; }
  /* #2339 PR 10 §5.3 — formula-with-no-computed_value fallback: mono, dim
     fg-3, NO corner wedge (ported verbatim from sheets.css). */
  .sh-c--raw { font-family: var(--font-mono); font-size: 11.5px; color: var(--fg-3); }
  /* #2339 PR 11 ShMismatch — amber corner wedge (top-right, distinct from
     `.sh-w--err`'s bottom-left) + dotted underline, ported verbatim from
     docs/design/sheets/sheets.css. */
  .sh-w--mis { right: 0; top: 0; border-right: 5px solid var(--status-warning); border-bottom: 5px solid transparent; }
  .sh-c--mis .sh-c__v { color: var(--fg-2); text-decoration: underline; text-decoration-style: dotted; text-decoration-color: var(--status-warning); text-underline-offset: 3px; }
  .sh-c--mis { justify-content: flex-start; }
  /* Hover/select tooltip for a mismatched cell — CSS-only trigger (no JS
     hover-tracking state machine, mirrors PR 10's ghost-hover-tint
     pattern), but the tooltip's OWN shell is a fully self-contained
     popover style rather than reusing `.sh-pop` cross-component (Svelte
     scoped styles don't share between `SheetGrid.svelte` and
     `ShHeaderMenu.svelte` anyway). */
  .sh-mis-tip { position: absolute; left: 0; top: 100%; margin-top: 4px; z-index: 25; width: max-content; max-width: 240px; background: #1c1c20; border: 1px solid var(--border-strong); border-radius: 8px; box-shadow: var(--shadow-pop); padding: 8px 10px; pointer-events: none; }
  .sh-mis-tip__t { font-size: 12px; font-weight: 600; color: var(--status-warning); margin-bottom: 2px; }
  .sh-mis-tip__sub { font-size: 11px; color: var(--fg-4); line-height: 1.4; }

  .sh-c.is-anchor { z-index: 1; box-shadow: inset 0 0 0 1.5px var(--accent), var(--sh-c-border-shadow); }
  .sh-c.is-range { background: var(--accent-soft); }
  .sh-c.is-redge-t { border-top: 1.5px solid var(--accent); margin-top: -1px; }
  .sh-c.is-redge-b { border-bottom: 1.5px solid var(--accent); }
  .sh-c.is-redge-l { border-left: 1.5px solid var(--accent); margin-left: -1px; }
  .sh-c.is-redge-r { border-right: 1.5px solid var(--accent); }

  /* #2339 PR 14 — dashed cyan reference-range outline while a formula is
     being edited. Deliberately DASHED (vs. the solid `.sh-c.is-redge-*`
     selection outline above) so the two can never be confused (design
     annotation, Matt ruling 2026-09-05 #7's surrounding context).
     Ported verbatim from docs/design/sheets/sheets.css. */
  .sh-c.is-ref { background: rgba(0, 228, 228, 0.07); }
  .sh-c.is-refedge-t { border-top: 1.5px dashed var(--accent); }
  .sh-c.is-refedge-b { border-bottom: 1.5px dashed var(--accent); }
  .sh-c.is-refedge-l { border-left: 1.5px dashed var(--accent); }
  .sh-c.is-refedge-r { border-right: 1.5px dashed var(--accent); }

  .sh-h--ghost { background: transparent; border-right-color: var(--border-faint); border-bottom-color: var(--border-faint); opacity: 0.55; }
  .sh-h--ghost .sh-h__ref { color: var(--fg-4); }
  .sh-h--ghost:hover { background: var(--overlay-hover); opacity: 1; }
  .sh-c--ghost { background: transparent; border-right-color: rgba(240, 237, 232, 0.035); border-bottom-color: rgba(240, 237, 232, 0.035); opacity: 0.55; }
  .sh-c--ghost:hover { background: var(--overlay-hover); opacity: 1; }
  /* #2339 PR 10 §5.4 carry-forward fix — a typing/mid-ghost-commit ghost
     cell must render at FULL opacity (the design's own is-typing override
     has no opacity property, so it doesn't reset the inherited 0.55 from
     `.sh-c--ghost` unless we say so explicitly here). */
  .sh-c--ghost.is-typing { background: var(--surface-1); box-shadow: inset 0 0 0 1.5px var(--accent), var(--sh-c-border-shadow); color: var(--fg-1); opacity: 1; }
  .sh-g--ghost { background: transparent; border-bottom-color: rgba(240, 237, 232, 0.035); color: var(--fg-4); opacity: 0.55; }

  .sh-loading, .sh-error-banner { padding: 24px; color: var(--fg-3); font-size: 13px; }
  .sh-error-banner { color: var(--status-danger); }

  /* PR 10 §3 — inline formula-rejection / commit-failure error text under
     an open editor. Not a design-specified element (the package doesn't
     design error states for this path, per scope §4/§10.8) — styled to
     match the existing `.sh-ed__hint` treatment (sheets.css) but in the
     danger color. */
  .sh-ed__err {
    position: absolute;
    left: 0;
    top: 100%;
    margin-top: 4px;
    white-space: nowrap;
    font-size: 11px;
    color: var(--status-danger);
    background: var(--surface-3);
    border: 1px solid rgba(239, 68, 68, 0.35);
    border-radius: var(--radius-sm);
    padding: 3px 7px;
    z-index: 22;
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Ghost-commit / checkbox-toggle / rename failure toast (ruling 8) —
     mirrors Editor.svelte's `.error-toast` pattern (same positioning,
     animation, dismiss-button shape) rather than inventing new toast
     styling, scoped under `sh-` naming to match this component's own
     convention. */
  .sh-toast {
    position: absolute;
    bottom: 16px;
    right: 16px;
    background: rgba(239, 68, 68, 0.12);
    border: 1px solid rgba(239, 68, 68, 0.35);
    color: #f87171;
    padding: 10px 14px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 40;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 320px;
    animation: shToastIn 0.2s ease;
  }
  .sh-toast__msg { flex: 1; line-height: 1.4; }
  .sh-toast__dismiss { background: none; border: none; color: #f87171; cursor: pointer; font-size: 14px; padding: 0; opacity: 0.7; flex-shrink: 0; }
  .sh-toast__dismiss:hover { opacity: 1; }
  @keyframes shToastIn {
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
  }
</style>
