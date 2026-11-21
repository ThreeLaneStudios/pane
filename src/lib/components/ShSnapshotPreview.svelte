<script lang="ts">
  // ShSnapshotPreview.svelte — the read-only mini-grid preview for the
  // sheet-native version-history panel (#2339 PR 15b-beta,
  // `docs/design/sheets/sheets-surfaces.jsx:150-176`, `sheets.css:282-308`
  // `.sh-ro`/`.sh-w--calc` rules). Deliberately its OWN lightweight
  // renderer — NOT a `SheetGrid` prop/mode — since `SheetGrid.svelte`'s
  // interactive machinery (selection, editors, drag-resize, formula bar
  // wiring) has nothing to do with rendering a frozen historical
  // envelope, and bolting a "read-only" branch through every one of
  // `SheetGrid`'s ~2000 lines would be far riskier than a small dedicated
  // component that only knows how to draw a grid.
  //
  // Visual spec (design + task brief, both matched exactly):
  //   - 34px header row, 32px data rows (independent of the live editor's
  //     own `HEAD_H=38`/row-height values — this preview has its own
  //     fixed metrics).
  //   - Header type glyphs via the SHARED `typeGlyph()` (sheetCellRender.ts
  //     — same glyph mapping SheetGrid's live headers use).
  //   - Values at `fg-2` (one step down from the live editor's `fg-1`),
  //     per the design's "read-only is expressed by removal" annotation.
  //   - Formula cells carry NO visual indicator (#2874 item 55 removed
  //     the corner-wedge/cyan-border-left treatment this bullet used to
  //     describe, per Matt's ruling: "no formula indicators anywhere" —
  //     users add borders/backgrounds themselves if they want them). A
  //     formula cell is visually indistinguishable from a plain-value
  //     cell here except for its rendered content (see below).
  //   - Cell styling (`style_json`) is preserved exactly, via
  //     `parseCellStyleJson` — inline `style:` attrs, since every
  //     snapshot's styling is arbitrary per-cell data, not a fixed class.
  //   - No ghost columns/rows, no selection ring, no hover states on
  //     headers/gutter/cells — nothing here has a click handler at all.
  //   - Formula cells render their COMPUTED VALUE (e.g. `=B1*2` renders
  //     as `42`, `=A1/0` renders as `#DIV/0!`) — item 32 (#2874): the
  //     backend's `get_sheet_snapshot_computed_values` command recomputes
  //     a historical envelope's formula cells for DISPLAY ONLY (never
  //     written back to `note_versions.content` — ADR-0066 D4 remains
  //     fully intact). If no computed entry exists for a formula cell yet
  //     (in-flight fetch, fetch failure, or genuinely missing), the cell
  //     renders EMPTY with the formula wedge indicator still shown —
  //     NEVER the raw stored formula string, and NEVER an internal
  //     column/row ID, under any circumstance (Matt's ruling 2026-09-15).

  // #2339 PR 15b-alpha (alpha): `cellStyle()` used to be a private
  // duplicate of the shared `cellInlineStyle()` helper below — same
  // bg/fg/border decl-building logic, byte-identical work. Now imports
  // the shared function (sheetCellRender.ts) instead of keeping its own
  // copy. `kind` is always passed as `'normal'` here since this preview
  // never classifies cells into calc/err/raw/mis kinds (it renders raw
  // formula strings, never computed/error values — see the file doc
  // comment above) — so the shared function's error-ink carve-out is
  // simply inert for every call site in this component, not bypassed.
  import { typeGlyph, cellInlineStyle, currencyCode, formatCurrencyValue } from '$lib/utils/sheetCellRender';
  import {
    parseSheetSnapshotEnvelope,
    pivotSnapshotEnvelope,
  } from '$lib/utils/sheetSnapshotEnvelope';

  interface Props {
    /** Raw `note_versions.content` JSON for a sheet snapshot row. */
    content: string;
    /** Computed display values for this snapshot's formula cells, keyed
     *  `"{column_id}:{row_id}"` — from `api.getSheetSnapshotComputedValues`
     *  (item 32, #2874). Absent/empty is a valid, fully-supported state
     *  (e.g. while the fetch is in flight, or if it failed): every
     *  formula cell without a matching entry renders EMPTY (#2874 item 55
     *  removed the formula wedge that used to still render in this case —
     *  there is no visual indicator anymore, empty is just empty) —
     *  NEVER the raw stored formula string. */
    computedValues?: Record<string, string>;
  }

  let { content, computedValues = {} }: Props = $props();

  /** Item 32 (#2874) fallback contract, per Matt's ruling (2026-09-15):
   *  - Non-formula cell -> always its raw stored value, unchanged.
   *  - Formula cell WITH a matching computed entry -> the computed value
   *    (a plain number/text/boolean rendering, or an error string like
   *    `#DIV/0!`).
   *  - Formula cell with NO computed entry (in-flight, failed, or
   *    missing) -> EMPTY STRING. #2874 item 55 removed this component's
   *    formula-wedge indicator entirely, so there is no longer anything
   *    visually distinguishing an unresolved formula cell from a blank
   *    one — but the text itself is still never the raw stored string
   *    (which could contain a raw internal `{column_id:row_id}`
   *    reference) and never a placeholder that could be mistaken for
   *    computed data. */
  function cellText(rawValue: string, colId: string, rowId: string): string {
    if (!rawValue.startsWith('=')) return rawValue;
    const entry = computedValues[`${colId}:${rowId}`];
    return entry ?? '';
  }

  /** #2874 item 38 (Matt's ruling 2026-09-15, ruling 3): currency cells in
   *  the snapshot preview use the SAME `formatCurrencyValue()` formatter
   *  as the live grid — no duplicated currency-formatting logic. Applies
   *  on top of `cellText()`'s existing formula-resolution contract
   *  (a formula cell's computed value, or empty string if unresolved, is
   *  what gets formatted — never the raw `=...` formula text). An
   *  unparseable value (e.g. an error string like `#DIV/0!`, or an empty
   *  string) passes through `formatCurrencyValue()`'s graceful fallback
   *  unchanged. */
  function currencyCellText(rawValue: string, colId: string, rowId: string, optionsJson: string): string {
    return formatCurrencyValue(cellText(rawValue, colId, rowId), optionsJson);
  }

  let envelope = $derived(parseSheetSnapshotEnvelope(content));
  let pivoted = $derived(envelope ? pivotSnapshotEnvelope(envelope) : null);

  function alignClass(colType: string): string {
    if (colType === 'number' || colType === 'currency') return 'sh-prev__c--num';
    return '';
  }


</script>

{#if !pivoted}
  <div class="sh-prev__unreadable" data-testid="sheet-snapshot-preview-unreadable">
    Couldn't read this snapshot.
  </div>
{:else}
  <div
    class="sh-prev"
    style:grid-template-columns={`40px repeat(${pivoted.columns.length}, minmax(80px, 1fr))`}
    data-testid="sheet-snapshot-preview"
  >
    <div class="sh-prev__corner" style:height="34px"></div>
    {#each pivoted.columns as column (column.id)}
      <div class="sh-prev__h" style:height="34px" data-testid="sheet-snapshot-preview-header">
        <span class="sh-prev__h-type">{typeGlyph(column.col_type)}</span>
        <span class="sh-prev__h-name">{column.name}</span>
        {#if column.col_type === 'currency'}
          <span class="sh-prev__h-code">{currencyCode(column.options_json)}</span>
        {/if}
      </div>
    {/each}

    {#each pivoted.rows as denseRow, ri (denseRow.row.id)}
      <div class="sh-prev__g" style:height="{denseRow.height}px">{ri + 1}</div>
      {#each denseRow.cells as dc, ci (pivoted.columns[ci].id)}
        <div
          class="sh-prev__c {alignClass(pivoted.columns[ci].col_type)}"
          style:height="{denseRow.height}px"
          style={dc.cell ? cellInlineStyle(dc.cell.style_json, 'normal') : ''}
          data-testid="sheet-snapshot-preview-cell-{ri}-{ci}"
        >{pivoted.columns[ci].col_type === 'currency'
            ? currencyCellText(dc.cell?.value ?? '', pivoted.columns[ci].id, denseRow.row.id, pivoted.columns[ci].options_json)
            : cellText(dc.cell?.value ?? '', pivoted.columns[ci].id, denseRow.row.id)}</div>
      {/each}
    {/each}
  </div>
{/if}

<style>
  .sh-prev { display: grid; background: var(--bg); }
  .sh-prev__unreadable { padding: 24px; font-size: 12.5px; color: var(--fg-4); text-align: center; }
  .sh-prev__corner { border-bottom: 1px solid var(--border-faint); border-right: 1px solid var(--border-faint); background: var(--surface-1); }
  .sh-prev__h {
    display: flex; align-items: center; gap: 5px; padding: 0 8px;
    border-bottom: 1px solid var(--border-faint); border-right: 1px solid var(--border-faint);
    background: var(--surface-1); font-size: 11.5px; font-weight: 500; color: var(--fg-3);
    box-sizing: border-box; overflow: hidden; cursor: default;
  }
  .sh-prev__h-type { color: var(--fg-4); font-family: var(--font-mono); font-size: 10px; flex-shrink: 0; }
  .sh-prev__h-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sh-prev__h-code { color: var(--fg-4); font-family: var(--font-mono); font-size: 9px; flex-shrink: 0; }
  .sh-prev__g {
    display: flex; align-items: center; justify-content: center;
    border-bottom: 1px solid var(--border-faint); border-right: 1px solid var(--border-faint);
    background: var(--surface-1); font-size: 11px; color: var(--fg-4); box-sizing: border-box; cursor: default;
  }
  .sh-prev__c {
    --sh-c-border-shadow: 0 0 0 0 transparent;
    display: flex; align-items: center; padding: 0 8px;
    border-bottom: 1px solid var(--border-faint); border-right: 1px solid var(--border-faint);
    font-size: 12.5px; color: var(--fg-2); box-sizing: border-box; overflow: hidden;
    white-space: nowrap; text-overflow: ellipsis; cursor: default;
    box-shadow: var(--sh-c-border-shadow);
  }
  .sh-prev__c--num { justify-content: flex-end; font-variant-numeric: tabular-nums; }
  /* #2874 item 55: the computed-formula wedge (`.sh-prev__c--calc`'s
     45%-opacity cyan border-left) was removed per Matt's ruling — no
     formula indicators anywhere, here or in the live grid. The
     `isFormula` class binding is gone too (nothing else consumed it).
     A formula cell still renders its computed value exactly as before
     (`cellText()`/`currencyCellText()`, unchanged) — only the visual
     marker is gone. */
</style>
