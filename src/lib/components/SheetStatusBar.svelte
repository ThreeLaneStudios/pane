<script lang="ts">
  // SheetStatusBar.svelte — cols/rows counts footer for the Sheets grid
  // (#2339 PR 9). `formulas`/`paused` (#2339 PR 14, Matt ruling 2026-09-05
  // #4 — bundled into the formula-bar PR alongside the `ShPaused` banner)
  // are wired from `SheetData.formula_count`/`formulas_paused` — the
  // design's `sh-status__calc` segment only renders when a count is
  // supplied, and "computation paused" only renders when `paused` is true
  // (`docs/design/sheets/sheets-shared.jsx`'s `StatusBar`).
  interface Props {
    cols: number;
    rows: number;
    formulas?: number;
    paused?: boolean;
  }

  let { cols, rows, formulas, paused }: Props = $props();
</script>

<div class="sh-status" data-testid="sheet-status-bar">
  <b>{cols}</b> columns<span>·</span><b>{rows}</b> rows
  {#if formulas != null}
    <span>·</span>
    <span class="sh-status__calc"><span class="sh-status__wedge"></span><b>{formulas}</b> computed</span>
  {/if}
  {#if paused}
    <span>·</span>
    <span class="sh-status__paused" data-testid="sheet-status-paused">computation paused</span>
  {/if}
  <span class="sh-status__sp"></span>
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css's status-bar section. */
  .sh-status { display: flex; align-items: center; gap: 12px; padding: 9px 18px; border-top: 1px solid var(--border-faint); flex-shrink: 0; font-size: 11.5px; color: var(--fg-4); }
  .sh-status b { color: var(--fg-3); font-weight: 400; font-variant-numeric: tabular-nums; }
  .sh-status__sp { flex: 1; }
  .sh-status__calc { display: inline-flex; align-items: center; gap: 5px; color: var(--fg-3); }
  .sh-status__wedge { width: 0; height: 0; border-left: 5px solid var(--accent); border-top: 5px solid transparent; }
  .sh-status__paused { color: var(--fg-3); }
</style>
