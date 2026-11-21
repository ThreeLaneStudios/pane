<script lang="ts">
  // ShGhostCellContextMenu.svelte — the Sheets grid's right-click context
  // menu for GHOST cells (#2874 item 22). A NEW component, not a `mode`
  // prop on `ShCellContextMenu.svelte` — see the item 22 scope doc for the
  // full rationale (near-zero row overlap: 2 of 6 rows; different
  // semantic domain: ghost cells have no contents to style/clear/export/
  // delete; the boundary-flip math is coupled to a hardcoded row-count-
  // derived MENU_HEIGHT that shouldn't be shared/switched between two very
  // differently-sized menus). Same precedent `ShCellContextMenu` itself
  // followed vs. `ShSheetMenu` (Matt's 2026-09-05 ruling), one level down.
  //
  // Same "dumb popover shell with callback props" division of labor as
  // `ShCellContextMenu.svelte`/`ShSheetMenu.svelte`/`ShHeaderMenu.svelte`:
  // owns its own open/close-on-outside-click/Escape lifecycle and
  // boundary-flip positioning (anchored off the right-click coordinate,
  // same as `ShCellContextMenu`), but every verb is a plain callback prop.
  //
  // Ghost cells have no contents (no styling, nothing to clear/export/
  // delete) — only "Insert row above" / "Insert column left" make sense.
  // The caller (`SheetGrid.svelte`) wires these callbacks to call
  // `insertRow()`/`insertColumn()` with ZERO arguments (append via
  // MAX(sort_order) path) rather than resolving a reference id + position
  // — passing `(undefined, 'before')` for a ghost coordinate hits a Rust
  // backend guard that rejects mismatched reference/position pairs.
  interface Props {
    x: number;
    y: number;
    onInsertRow: () => void;
    onInsertColumn: () => void;
    onClose: () => void;
  }

  let { x, y, onInsertRow, onInsertColumn, onClose }: Props = $props();

  let menuEl: HTMLDivElement | null = $state(null);

  const MENU_WIDTH = 240;
  // 2 rows, no dividers — much smaller and more accurate than
  // `ShCellContextMenu`'s 9-row/328px estimate. No guessing needed.
  const MENU_HEIGHT = 96;

  let flipLeft = $derived.by(() => {
    const viewportWidth = typeof window !== 'undefined' && (window as any).innerWidth ? (window as any).innerWidth : Infinity;
    return x + MENU_WIDTH > viewportWidth;
  });
  let flipUp = $derived.by(() => {
    const viewportHeight = typeof window !== 'undefined' && (window as any).innerHeight ? (window as any).innerHeight : Infinity;
    return y + MENU_HEIGHT > viewportHeight;
  });

  let posLeft = $derived(flipLeft ? x - MENU_WIDTH : x);
  let posTop = $derived(flipUp ? y - MENU_HEIGHT : y);

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

  function handleInsertRowClick() {
    onInsertRow();
    onClose();
  }

  function handleInsertColumnClick() {
    onInsertColumn();
    onClose();
  }

  $effect(() => {
    if (menuEl) menuEl.focus();
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={menuEl}
  class="sh-pop sh-gctx"
  style:top="{posTop}px"
  style:left="{posLeft}px"
  role="menu"
  tabindex="-1"
  aria-label="Ghost cell context menu"
  onkeydown={handleKeydown}
  onclick={(e) => e.stopPropagation()}
  data-testid="ghost-cell-context-menu"
>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleInsertRowClick} data-testid="ghost-cell-context-menu-insert-row">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="6" rx="1.6"/><path d="M3 16h18M12 13.5v5"/></svg></span>Insert row above
  </button>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleInsertColumnClick} data-testid="ghost-cell-context-menu-insert-column">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="3" width="6" height="18" rx="1.6"/><path d="M16 3v18M13.5 12h5"/></svg></span>Insert column left
  </button>
</div>

<style>
  /* Shared `.sh-pop*` box-model/typography rules live globally in
     src/app.css — this component keeps only its own width + position
     overrides, same pattern as `ShCellContextMenu.svelte`. */
  .sh-gctx { position: fixed; width: 240px; }
</style>
