<script lang="ts">
  // ShCellContextMenu.svelte — the Sheets grid's right-click cell/range
  // context menu (#2339 PR 15b-alpha (alpha), scope report §3.1/§4).
  // A NEW component, not a `mode` prop on `ShSheetMenu.svelte` — see the
  // scope report §3.1 for the full rationale (different trigger: click vs.
  // contextmenu; different domain: sheet-object actions vs. cell/range
  // actions; near-zero row overlap between the two menus' item sets).
  // Endorsed by Matt's 2026-09-05 ruling: "New `ShCellContextMenu` on
  // shared popover shell (not extending `ShSheetMenu`)."
  //
  // Same "dumb popover shell with callback props" division of labor as
  // `ShSheetMenu.svelte`/`ShHeaderMenu.svelte`: this component owns its
  // own open/close-on-outside-click/Escape lifecycle and boundary-flip
  // positioning, but every verb is a plain callback prop — all the actual
  // work (opening the style panel, clearing styling, inserting a row/
  // column, clearing cell contents) lives in the caller (`SheetGrid.svelte`),
  // which already owns every one of those mechanisms.
  //
  // KEY DIFFERENCE from `ShSheetMenu`/`ShHeaderMenu`: this menu anchors off
  // a RIGHT-CLICK COORDINATE (`x`/`y`, the triggering `MouseEvent`'s
  // `clientX`/`clientY`), not a static, persistent button ref — the first
  // popover in the codebase whose anchor point moves per-invocation
  // (scope report §3.1/§6, "New context-menu positioning math"). Boundary-
  // flip math therefore takes the click coordinate directly as an input,
  // rather than measuring `anchorEl.getBoundingClientRect()`.
  //
  // 7-element design order (design source: sheets-formula.jsx L264-276;
  // Export sheet as CSV… row removed per #2874 item 11 — ShSheetMenu.svelte
  // keeps its own independent Export CSV row, zero shared code path.
  // #2874 item 31: "Insert column left" moved to `ShHeaderMenu.svelte`'s
  // column-operations group (above Sort ascending) — removed from here
  // entirely, not merely duplicated. `ShGhostCellContextMenu` is untouched
  // and keeps its own independent insert-column row):
  //   1. Style cells… (⌘⇧S hint)   → onStyleCells
  //   2. Clear styling              → onClearStyling
  //   3. hr
  //   4. Insert row above           → onInsertRow
  //   5. hr
  //   6. Delete N cells' contents (danger) → onDeleteContents
  //
  // Alpha (this PR) wires everything EXCEPT the style panel itself — see
  // `SheetGrid.svelte`'s `onStyleCells` call site, which just closes the
  // menu for now ("beta wires this", per the task brief). This component
  // has no knowledge of that deferral; `onStyleCells` is a plain callback
  // like every other row here.
  interface Props {
    x: number;
    y: number;
    /** Number of cells the active selection covers — drives the danger
     *  row's "Delete N cells' contents" label. 1 for a bare anchor. */
    cellCount: number;
    onStyleCells: () => void;
    onClearStyling: () => void;
    onInsertRow: () => void;
    onDeleteContents: () => void;
    onClose: () => void;
  }

  let { x, y, cellCount, onStyleCells, onClearStyling, onInsertRow, onDeleteContents, onClose }: Props = $props();

  let menuEl: HTMLDivElement | null = $state(null);

  const MENU_WIDTH = 240;
  // No fixed menu height is measured off the DOM (same posture
  // `ShSheetMenu`/`ShHeaderMenu` take for width — a static estimate,
  // since the row count here is fixed at build time, not data-driven).
  // 6 rows + 2 hrs + padding, measured against the real rendered
  // component during design review. Recalculated for #2874 item 31:
  // the prior 285px estimate (7 rows + 2 hrs) covered one more button
  // row than the current layout ("Insert column left", moved to
  // `ShHeaderMenu.svelte` and removed here entirely, with no hr removed
  // alongside it), so 285 is reduced by one bare-row unit (~28px) to 257.
  const MENU_HEIGHT = 257;

  let flipLeft = $derived.by(() => {
    const viewportWidth = typeof window !== 'undefined' && (window as any).innerWidth ? (window as any).innerWidth : Infinity;
    return x + MENU_WIDTH > viewportWidth;
  });
  let flipUp = $derived.by(() => {
    const viewportHeight = typeof window !== 'undefined' && (window as any).innerHeight ? (window as any).innerHeight : Infinity;
    return y + MENU_HEIGHT > viewportHeight;
  });

  let posLeft = $derived(flipLeft ? x - MENU_WIDTH : x);
  // #2874 item 57 (Matt's ruling): floor the flipped position at 0 so a
  // mid-screen flip-up on a short window can never clip off the TOP of
  // the window — same shape as the horizontal clamp above (`Math.max(0,
  // …)` wrapping the overflow expression, applied only on the overflow
  // branch). Only the flipped branch gets the floor; the non-flipped `y`
  // position is already correct.
  let posTop = $derived(flipUp ? Math.max(0, y - MENU_HEIGHT) : y);

  function handleWindowClick(e: MouseEvent) {
    if (e.button !== 0) return;
    if (e.target instanceof Node && !e.target.isConnected) return;
    const target = e.target as Node;
    if (menuEl && menuEl.contains(target)) return;
    onClose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  }

  function handleStyleCellsClick() {
    onStyleCells();
    onClose();
  }

  function handleClearStylingClick() {
    onClearStyling();
    onClose();
  }

  function handleInsertRowClick() {
    onInsertRow();
    onClose();
  }

  function handleDeleteContentsClick() {
    onDeleteContents();
    onClose();
  }

  $effect(() => {
    if (menuEl) menuEl.focus();
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={menuEl}
  class="sh-pop sh-cctx"
  style:top="{posTop}px"
  style:left="{posLeft}px"
  role="menu"
  tabindex="-1"
  aria-label="Cell context menu"
  onkeydown={handleKeydown}
  onclick={(e) => e.stopPropagation()}
  data-testid="cell-context-menu"
>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleStyleCellsClick} data-testid="cell-context-menu-style">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M11 3l10 10-7.6 7.6a2.4 2.4 0 0 1-3.4 0L3 13.4a2.4 2.4 0 0 1 0-3.4L11 3z"/><path d="M20 16.5c1.3 1.8 1.3 3.4 0 4.4-1.3 1-3 .6-3.6-.8-.6-1.3.6-2.7 3.6-3.6z"/></svg></span>Style cells…<span class="sh-pop__kb">⌘⇧S</span>
  </button>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleClearStylingClick} data-testid="cell-context-menu-clear-styling">
    <span class="sh-pop__ico"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/></svg></span>Clear styling
  </button>
  <div class="sh-pop__hr"></div>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleInsertRowClick} data-testid="cell-context-menu-insert-row">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="6" rx="1.6"/><path d="M3 16h18M12 13.5v5"/></svg></span>Insert row above
  </button>
  <div class="sh-pop__hr"></div>
  <button type="button" class="sh-pop__i sh-pop__danger" role="menuitem" onclick={handleDeleteContentsClick} data-testid="cell-context-menu-delete-contents">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></svg></span>Delete {cellCount} cell{cellCount === 1 ? "'s" : "s'"} contents
  </button>
</div>

<style>
  /* #2339 PR 15b-alpha: shared `.sh-pop*` box-model/typography rules live
     globally in src/app.css — this component keeps only its own width +
     position overrides, same pattern as `ShSheetMenu.svelte`/
     `ShHeaderMenu.svelte`. Unlike those two, `top`/`left` are set via
     inline style (JS-computed from the click coordinate, not CSS), since
     the anchor point is dynamic per-invocation rather than a fixed
     corner of a static button. 240px — slightly wider than
     `ShSheetMenu`'s 214px — to fit the right-aligned "⌘⇧S" shortcut hint
     on the first row without wrapping. */
  .sh-cctx { position: fixed; width: 240px; }
</style>
