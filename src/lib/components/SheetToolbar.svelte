<script lang="ts">
  // SheetToolbar.svelte — the sheet's `.etool` toolbar row shell.
  //
  // PR 9 shipped every button laid out exactly as the active toolbar would
  // be, but `disabled` — no layout shift once handlers get wired up.
  // PR 10 (#2339, ruling 5) wires the first two: insert-row/insert-column,
  // calling straight through to the same `create_sheet_row`/
  // `create_sheet_column` calls ghost-commit already uses on SheetGrid,
  // via the `onInsertRow`/`onInsertColumn` callback props below. Every
  // other button remains `disabled` — out of scope for PR 10. Follows
  // kit.css's `.etool`/`.ebtn`/`.esep` conventions (docs/design/sheets/
  // kit.css lines ~430-448) and the structure/style/formula/view/overflow
  // button order from docs/design/sheets/sheets-shared.jsx's `Toolbar`
  // component.

  // #2339 PR 15b-alpha: wires the previously-disabled "More" button into
  // a `ShSheetMenu.svelte` overflow popover (Export as CSV… / Version
  // history / Delete sheet — "Move to…" was removed, #2874 item 49, the
  // folder pill already covers it). This component owns the
  // button's own open/anchor state (same "local popover owns its own
  // open state" pattern `SheetGrid.svelte` uses for `ShHeaderMenu`'s
  // `openHeaderMenu`) — every verb itself is a plain callback prop,
  // implemented by `Editor.svelte` (the same file that already owns
  // version history / the folder picker / move-to-trash for every other
  // note type). "Rename sheet" was removed (#2874 item 33) — the sheet
  // title is already a click-to-edit input, so no dedicated rename verb
  // is needed here.
  import ShSheetMenu from '$lib/components/ShSheetMenu.svelte';

  // #2339 PR 15d-beta: un-disables the style buttons, wired to plain
  // callback props — Matt's ruling 1 ("Selection threading → callback
  // pattern", scope report): this toolbar stays completely ignorant of
  // what's currently selected. The button just fires its callback;
  // `SheetGrid.svelte` (via `Editor.svelte`'s `bind:this` wiring) is the
  // one place that knows the current selection and opens `ShStylePanel`
  // in response.
  //
  // #2874 item 16 (Parts A/B/C): the style callback prop passes the
  // CLICKING button's own `getBoundingClientRect()` — same `bind:this`
  // capture pattern as `moreBtnEl` below — so `Editor.svelte` (→
  // `SheetGrid.svelte`'s `openStylePanel()`) can anchor the style panel
  // under the real button instead of a hardcoded fallback corner (Part B).
  // Version history/sort/filter/formula are un-disabled and wired to new
  // plain callback props, same "toolbar stays ignorant of selection"
  // discipline — `sortEnabled`/`filterEnabled`/`formulaEnabled` are the
  // only selection-DERIVED state this toolbar receives, and even those are
  // plain booleans computed elsewhere (`SheetGrid.svelte`/`Editor.svelte`),
  // never raw selection data.
  //
  // #2874 item 20: the three separate fill/text/border buttons are
  // replaced with a single unified "Style cells" button (bucket+droplet
  // glyph, matching `ShCellContextMenu.svelte`'s "Style cells…" row) —
  // this also closes item 13 (the old fill button's glyph was missing the
  // droplet detail). `openStylePanel()` already exposes every fill/text/
  // border control in one popover, so three separate entry points were
  // redundant. `onStyleClick` replaces `onFillClick`/`onTextClick`/
  // `onBorderClick`; `fillSwatch` is dropped since a single merged glyph
  // has no one "current value" to preview.
  interface Props {
    onInsertRow?: () => void;
    onInsertColumn?: () => void;
    onExportCsv?: () => void;
    onVersionHistory?: () => void;
    onDeleteSheet?: () => void;
    onStyleClick?: (anchorEl?: HTMLElement) => void;
    onFormulaClick?: () => void;
    onSortClick?: () => void;
    onFilterClick?: (anchorEl?: HTMLElement) => void;
    /** Disables the Sort button when no column can be resolved (an empty
     *  sheet with 0 real columns) — mirrors `menuSort`'s own no-op guard
     *  one level up, at the button, rather than only inside the engine. */
    sortEnabled?: boolean;
    /** Disables the Filter button when the selected column's type doesn't
     *  support filtering (`filterSupported()`, `sheetView.ts`) — same
     *  ruling-3 gate the header menu's own "Filter…" row already applies,
     *  reapplied here one level up. */
    filterEnabled?: boolean;
    /** Disables the Formula (fx) button when nothing is selected — mirrors
     *  `openFormulaOnAnchor()`'s own no-op guard (only reachable on a
     *  genuinely empty 0-row/0-column sheet). */
    formulaEnabled?: boolean;
  }

  let {
    onInsertRow,
    onInsertColumn,
    onExportCsv,
    onVersionHistory,
    onDeleteSheet,
    onStyleClick,
    onFormulaClick,
    onSortClick,
    onFilterClick,
    sortEnabled = true,
    filterEnabled = true,
    formulaEnabled = true,
  }: Props = $props();

  let moreMenuOpen = $state(false);
  let moreBtnEl: HTMLButtonElement | null = $state(null);
  let styleBtnEl: HTMLButtonElement | null = $state(null);
  let filterBtnEl: HTMLButtonElement | null = $state(null);

  function toggleMoreMenu() {
    moreMenuOpen = !moreMenuOpen;
  }

  function closeMoreMenu() {
    moreMenuOpen = false;
  }
</script>

<div class="etool sh-tb" data-testid="sheet-toolbar" role="toolbar" aria-label="Sheet toolbar">
  <button class="ebtn" onclick={() => onInsertColumn?.()} title="Insert column" aria-label="Insert column" data-testid="sheet-tb-insert-column">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="3" width="6" height="18" rx="1.6"/><path d="M16 3v18M13.5 12h5"/></svg>
  </button>
  <button class="ebtn" onclick={() => onInsertRow?.()} title="Insert row" aria-label="Insert row" data-testid="sheet-tb-insert-row">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="6" rx="1.6"/><path d="M3 16h18M12 13.5v5"/></svg>
  </button>
  <span class="esep"></span>
  <button class="ebtn" bind:this={styleBtnEl} onclick={() => onStyleClick?.(styleBtnEl ?? undefined)} title="Style cells" aria-label="Style cells" data-testid="sheet-tb-style">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M11 3l10 10-7.6 7.6a2.4 2.4 0 0 1-3.4 0L3 13.4a2.4 2.4 0 0 1 0-3.4L11 3z"/><path d="M20 16.5c1.3 1.8 1.3 3.4 0 4.4-1.3 1-3 .6-3.6-.8-.6-1.3.6-2.7 3.6-3.6z"/></svg>
  </button>
  <span class="esep"></span>
  <button class="ebtn" disabled={!formulaEnabled} onclick={() => onFormulaClick?.()} title="Formula" aria-label="Formula" data-testid="sheet-tb-formula">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 21c2.6 0 3.3-1.7 3.6-4L13 6.5C13.3 4.4 14.3 3.5 16.4 3.5H19M7.3 12h7.5M17 14l5 7M22 14l-5 7"/></svg>
  </button>
  <span class="esep"></span>
  <button class="ebtn" disabled={!sortEnabled} onclick={() => onSortClick?.()} title="Sort" aria-label="Sort" data-testid="sheet-tb-sort">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v16M6 14l6 6 6-6"/></svg>
  </button>
  <button class="ebtn" bind:this={filterBtnEl} disabled={!filterEnabled} onclick={() => onFilterClick?.(filterBtnEl ?? undefined)} title="Filter" aria-label="Filter" data-testid="sheet-tb-filter">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h18l-7 8v7l-4-2.5V12L3 4z"/></svg>
  </button>
  <span class="esep"></span>
  <button class="ebtn" onclick={() => onVersionHistory?.()} title="Version history" aria-label="Version history" data-testid="sheet-tb-history">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 16 14"/></svg>
  </button>
  <span class="sh-tb__sp"></span>
  <div class="sh-tb__more-wrap">
    <button
      class="ebtn"
      class:is-open={moreMenuOpen}
      bind:this={moreBtnEl}
      onclick={toggleMoreMenu}
      title="More"
      aria-label="More options"
      data-testid="sheet-tb-more"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="19" cy="12" r="1.5"/></svg>
    </button>
    {#if moreMenuOpen}
      <ShSheetMenu
        anchorEl={moreBtnEl}
        onExportCsv={() => onExportCsv?.()}
        onVersionHistory={() => onVersionHistory?.()}
        onDelete={() => onDeleteSheet?.()}
        onClose={closeMoreMenu}
      />
    {/if}
  </div>
</div>

<style>
  /* .etool / .ebtn / .esep authored fresh here, ported from
     docs/design/sheets/kit.css (~lines 430-448) — those classes don't yet
     exist anywhere in src/, this is the first use. */
  .etool {
    display: flex; align-items: center; gap: 4px;
    padding: 6px;
    background: var(--surface-1);
    border: 1px solid var(--border-faint);
    border-radius: 8px;
    width: fit-content;
  }
  .ebtn {
    background: transparent; border: 0;
    color: var(--fg-3);
    font-family: var(--font-sans); font-size: 13px; font-weight: 500;
    padding: 5px 10px; border-radius: 5px;
    cursor: pointer;
    min-width: 28px;
    display: inline-flex; align-items: center; justify-content: center;
  }
  .ebtn:hover:not(:disabled) { background: var(--surface-2); color: var(--fg-1); }
  .ebtn:disabled { cursor: default; opacity: 0.55; }
  .esep { width: 1px; height: 16px; background: var(--border-faint); margin: 0 4px; }

  /* #2339 PR 15b-alpha — More button's open state (matches
     `ShHeaderMenu`'s `.sh-h.is-open` convention: active accent ring while
     its own popover is open) + the positioned wrapper `ShSheetMenu`
     anchors off. */
  .sh-tb__more-wrap { position: relative; }
  .ebtn.is-open { background: var(--surface-2); color: var(--fg-1); box-shadow: inset 0 0 0 1px var(--accent); }

  /* Sheet-specific toolbar container + fill-swatch chip (sheets.css). */
  .sh-tb {
    /* .etool already provides the row chrome; sh-tb adds the sheet-row
       specific top/bottom hairlines matching the design's toolbar
       placement between the note header and the grid. */
    border-radius: 0;
    border-left: 0; border-right: 0;
    border-top: 1px solid var(--border-faint);
    border-bottom: 1px solid var(--border-faint);
    width: 100%;
    box-sizing: border-box;
    flex-shrink: 0;
  }
  .sh-tb__sp { flex: 1; }
</style>
