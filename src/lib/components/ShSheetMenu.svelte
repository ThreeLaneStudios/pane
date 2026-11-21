<script lang="ts">
  // ShSheetMenu.svelte — the sheet toolbar's "More" overflow menu (#2339
  // PR 15b-alpha, export menu, docs/design/sheets/sheets-surfaces.jsx
  // "ShExport" L248-278). Four rows on one `.sh-pop` popover: Export as
  // CSV… · Version history · hr · Delete sheet (danger) — matching the
  // design's own row order/grouping exactly ("first because it produces
  // something you take away… the destructive row is alone at the
  // bottom", design Cap prose). "Rename sheet" was removed (#2874
  // item 33) — the sheet title is already a click-to-edit input
  // (`Editor.svelte`), so a dedicated rename menu item was redundant.
  // "Move to…" was removed (#2874 item 49) — the folder pill in the
  // title row already covers moving a sheet between folders, and the
  // row was also non-functional (see `Editor.svelte`'s removed
  // `handleMoveToSheet`), so it was dead weight twice over.
  //
  // This is a dumb popover shell, same division of labor as
  // `ShHeaderMenu.svelte`: it owns its own open/close-on-outside-click/
  // Escape lifecycle, but every verb is a
  // plain callback prop — all the actual work (invoking the CSV export
  // command + save dialog, opening version history, moving to trash)
  // lives in `Editor.svelte`, which already owns every one of those
  // mechanisms for the rest of the note editor chrome. See that file's
  // own `handleExportCsv`/`openVersionHistory`/`handleMoveToTrash` for
  // what each callback actually does.
  interface Props {
    anchorEl: HTMLElement | null;
    onExportCsv: () => void;
    onVersionHistory: () => void;
    onDelete: () => void;
    onClose: () => void;
  }

  let { anchorEl, onExportCsv, onVersionHistory, onDelete, onClose }: Props = $props();

  let menuEl: HTMLDivElement | null = $state(null);

  function handleWindowClick(e: MouseEvent) {
    if (e.target instanceof Node && !e.target.isConnected) return;
    const target = e.target as Node;
    if (menuEl && menuEl.contains(target)) return;
    if (anchorEl && anchorEl.contains(target)) return;
    onClose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  }

  function handleExportClick() {
    onExportCsv();
    onClose();
  }

  function handleVersionHistoryClick() {
    onVersionHistory();
    onClose();
  }

  function handleDeleteClick() {
    onDelete();
    onClose();
  }

  $effect(() => {
    if (menuEl) menuEl.focus();
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={menuEl}
  class="sh-pop"
  role="menu"
  tabindex="-1"
  aria-label="Sheet menu"
  onkeydown={handleKeydown}
  onclick={(e) => e.stopPropagation()}
  data-testid="sheet-more-menu"
>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleExportClick} data-testid="sheet-more-menu-export">
    <span class="sh-pop__ico"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v11M7.5 10L12 14.5 16.5 10M4 20h16"/></svg></span>Export as CSV…
  </button>
  <button type="button" class="sh-pop__i" role="menuitem" onclick={handleVersionHistoryClick} data-testid="sheet-more-menu-history">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg></span>Version history
  </button>
  <div class="sh-pop__hr"></div>
  <button type="button" class="sh-pop__i sh-pop__danger" role="menuitem" onclick={handleDeleteClick} data-testid="sheet-more-menu-delete">
    <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></svg></span>Delete sheet
  </button>
</div>

<style>
  /* #2339 PR 15b-alpha: shared `.sh-pop*` box-model/typography rules live
     globally in src/app.css — this component keeps only its own width +
     positioning override. Unconditionally right-anchored to the More
     button's right edge (`.sh-tb__more-wrap`, already `position: relative`
     in SheetToolbar.svelte) — the button is always the toolbar's
     rightmost item, so this never needs to flip. Replaces the old
     `window.innerWidth`-based flip measurement, which was wrong when the
     chat panel is open (the real constraint is the editor pane's own
     right edge, clipped by `.editor-wrapper`'s `overflow: hidden`, not
     the full browser viewport). See #2874 item 5. */
  .sh-pop { width: 214px; top: calc(100% + 4px); left: auto; right: 0; }
</style>
