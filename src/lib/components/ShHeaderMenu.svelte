<script lang="ts">
  // ShHeaderMenu.svelte — the column header context menu (#2339 PR 11,
  // §33.3/§33.7, scope report
  // `docs/audits/2026-09-04-sheets-pr11-header-menu-scope.md`). Five verbs
  // on one `.sh-pop` popover: rename, type-change, sort asc/desc, filter,
  // delete. One panel with THREE internal view-states (`'root' | 'type' |
  // 'filter'`) that swap CONTENTS in place — never a second stacked
  // popover, matching the design's own `ShHeaderMenu` frame (`sheets-
  // grid.jsx`: the type submenu is a second `.sh-pop` shown alongside the
  // root menu in the design doc purely for illustration purposes; the real
  // component swaps one panel's children, since only one menu can
  // meaningfully be open at a time).
  //
  // Matt's ruling 2026-09-04 (1): Rename does NOT get an embedded input in
  // this popover, despite the locked design mockup showing one. Clicking
  // "Rename" here closes this menu and hands off to PR 10's EXISTING
  // inline `.sh-h__ren` header-rename input (`SheetGrid.svelte`'s own
  // `startHeaderRename`) — one rename mechanism, two entry points
  // (ghost-column-birth and this menu). `onRename` is a plain callback;
  // this component owns no rename input of its own.
  //
  // #2339 PR 15b-beta: ruling 2's carry-forward is now filled in. A
  // fourth view-state, `'options'`, swaps in `ShOptionsPanel.svelte` —
  // same in-place-swap pattern as `'type'`/`'filter'`, never a second
  // stacked popover (matching this file's own existing convention, and
  // the task brief's explicit "fourth view-state" wording over the
  // design mockup's illustrative side-by-side panels). The row itself is
  // visible ONLY for `select` columns, directly under Type, carrying the
  // live choice count.
  //
  // Ruling 3: the "Filter…" row is only interactive for `select`/
  // `checkbox` columns (`filterSupported`) — every other type renders it
  // `disabled` (visible, grayed, per the ruling's "user sees it exists but
  // can't use it yet"). Clicking it swaps to the `'filter'` view, a value-
  // picker list reusing the `.sh-pop__i`/`.sh-pop__tick` click-to-toggle
  // pattern from `SheetCellEditorSelect.svelte`. Unlike every other verb,
  // toggling a filter value is NOT terminal — the menu stays open so
  // multiple values can be selected in one sitting (`onFilterChange` fires
  // on every toggle, immediately, for a live-filtering feel) — only
  // Escape/outside-click/root-navigation actually closes the menu.

  import type { SheetColumn } from '$lib/types';
  // #2339 PR 12, Matt's ruling 2026-09-04 (3): `filterLabel` used to be a
  // private function in THIS file — it's relocated (mechanical move, zero
  // behavior change) into `sheetView.ts` so `SheetViewStrip`'s
  // `composeFilterChipText` can share the exact same label rule instead of
  // duplicating it. This is the ONLY change to this file for PR 12.
  import { selectChoicesFromOptionsJson, filterLabel, filterSupported } from '$lib/utils/sheetView';
  import ShOptionsPanel from '$lib/components/ShOptionsPanel.svelte';

  interface Props {
    column: SheetColumn;
    anchorEl: HTMLElement | null;
    /** The filter values CURRENTLY active for this column (mirrors
     *  `SheetViewState.filterValues` when `filterColumnId === column.id`),
     *  or `null` if this column isn't the active filter — drives which
     *  filter-picker rows render `.is-on`. */
    filterValues: string[] | null;
    /** #2874 item 16 Part C: when `'filter'`, the popover mounts straight
     *  into the filter value-picker view instead of the default root menu
     *  — the toolbar Filter button's entry point (`SheetGrid.svelte`'s
     *  `toolbarFilter()`), which reuses this ENTIRE component rather than
     *  building a second value-picker UI. Every existing header-click
     *  entry point passes no `initialView` (or `'default'`) and is
     *  unaffected — defaults to the pre-existing root-menu behavior. */
    initialView?: 'default' | 'filter';
    onRename: () => void;
    onTypeChange: (colType: string) => void;
    /** #2874 item 31: "Insert column left" moved here from
     *  `ShCellContextMenu.svelte` (removed there entirely, not
     *  duplicated) — grouped with Rename/Type above the divider that
     *  precedes Sort ascending, per Matt's ruling. Caller resolves the
     *  reference column id and wires it straight to `insertColumn`,
     *  same "dumb popover, plain callback prop" contract as every other
     *  verb in this file. `ShGhostCellContextMenu` is untouched and keeps
     *  its own independent insert-column row. */
    onInsertColumn: () => void;
    onSortAsc: () => void;
    onSortDesc: () => void;
    onFilterChange: (values: string[]) => void;
    onDelete: () => void;
    onClose: () => void;
    /** #2339 PR 15b-beta: fires with the full new `choices` array from
     *  `ShOptionsPanel` — the SAME "caller persists" contract as every
     *  other verb here (`onTypeChange` et al.); this component never
     *  calls `updateSheetColumn` itself. */
    onOptionsChange: (choices: string[]) => void;
  }

  let {
    column,
    anchorEl,
    filterValues,
    initialView = 'default',
    onRename,
    onTypeChange,
    onInsertColumn,
    onSortAsc,
    onSortDesc,
    onFilterChange,
    onDelete,
    onClose,
    onOptionsChange,
  }: Props = $props();

  type MenuView = 'root' | 'type' | 'filter' | 'options';
  // #2874 item 16 Part C: only actually starts in 'filter' when the
  // column supports it (`filterSupported`, same gate the root view's
  // "Filter…" row itself is disabled behind) — an `initialView: 'filter'`
  // request for an unsupported column type falls back to 'root' rather
  // than showing a filter view with nothing to pick.
  let view: MenuView = $state(initialView === 'filter' && filterSupported(column.col_type) ? 'filter' : 'root');
  let menuEl: HTMLDivElement | null = $state(null);

  /** Boundary-flip positioning: `right: 0` instead of `left: 0` when the
   *  popover's 208px width would overflow the viewport's right edge,
   *  measured off the trigger header cell's own rect (design spec, scope
   *  report positioning strategy). Computed once per mount off `anchorEl`
   *  — the header cell doesn't move while its own menu is open, so this
   *  doesn't need to be a live-tracking effect. */
  const MENU_WIDTH = 208;
  let flipRight = $state(false);
  $effect(() => {
    if (anchorEl && typeof anchorEl.getBoundingClientRect === 'function') {
      const rect = anchorEl.getBoundingClientRect();
      const viewportWidth = typeof window !== 'undefined' && (window as any).innerWidth ? (window as any).innerWidth : Infinity;
      flipRight = rect.left + MENU_WIDTH > viewportWidth;
    }
  });

  const TYPES: Array<[string, string]> = [
    ['text', 'Text'],
    ['number', 'Number'],
    ['currency', 'Currency'],
    ['date', 'Date'],
    ['checkbox', 'Checkbox'],
    ['select', 'Select'],
  ];

  /** Value-picker choices for the filter view. Checkbox is a fixed
   *  True/False pair (raw values `'1'`/`'0'`, matching the grid's own
   *  on/off predicate); select pulls from `options_json.choices`, same
   *  parse pattern as `SheetGrid.svelte`'s `selectChoicesFor` /
   *  `SheetCellEditorSelect.svelte`. Any other type (unreachable via the
   *  UI, since the row is `disabled`) returns no choices. */
  function filterChoicesFor(col: SheetColumn): string[] {
    if (col.col_type === 'checkbox') return ['1', '0'];
    if (col.col_type === 'select') return selectChoicesFromOptionsJson(col.options_json);
    return [];
  }

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
      if (view !== 'root') {
        view = 'root';
      } else {
        onClose();
      }
    }
  }

  function handleRenameClick() {
    onRename();
    onClose();
  }

  function handleTypeRowClick() {
    view = 'type';
  }

  function handleTypeChoice(colType: string) {
    if (colType !== column.col_type) onTypeChange(colType);
    onClose();
  }

  function handleInsertColumnClick() {
    onInsertColumn();
    onClose();
  }

  function handleSortAsc() {
    onSortAsc();
    onClose();
  }

  function handleSortDesc() {
    onSortDesc();
    onClose();
  }

  function handleFilterRowClick() {
    if (!filterSupported(column.col_type)) return;
    view = 'filter';
  }

  function handleOptionsRowClick() {
    view = 'options';
  }

  function handleOptionsChange(choices: string[]) {
    onOptionsChange(choices);
  }

  function handleOptionsBack() {
    view = 'root';
  }

  function toggleFilterValue(choice: string) {
    const current = filterValues ?? [];
    const next = current.includes(choice) ? current.filter((v) => v !== choice) : [...current, choice];
    onFilterChange(next);
  }

  function handleDeleteClick() {
    onDelete();
    onClose();
  }

  function typeLabel(colType: string): string {
    return TYPES.find(([t]) => t === colType)?.[1] ?? colType;
  }

  /** #2874 item 9: maps each column type to its `sh-pop__ico` markup for
   *  the type submenu. `text`/`number`/`currency` render as a monospace
   *  T/#/$ character (no SVG — matches the design frame's `TypeGlyph`
   *  letter branch, `sheets-shared.jsx:45-46`); `date`/`checkbox`/`select`
   *  render as inline SVGs using the same 24px viewBox / 1.7 stroke
   *  wrapper as every other icon in this file (matches `TypeGlyph`'s SVG
   *  branch, `sheets-shared.jsx:47-49`). Replaces the old `label[0]`
   *  first-letter derivation, which rendered the wrong glyph for every
   *  row (N/C/D/C/S instead of T/#/$/calendar/checkbox/list — see
   *  `docs/audits/2026-09-07-item9-rescope-v2.md` §4). `select` is also
   *  the fallback branch for any unrecognized type, matching `TypeGlyph`'s
   *  own fallback behavior in the design frame. */
  function typeGlyphMarkup(colType: string): string {
    const monoSpan = (ch: string) =>
      `<span style="font-family: var(--font-mono); font-size: 11px; font-weight: 500">${ch}</span>`;
    if (colType === 'text') return monoSpan('T');
    if (colType === 'number') return monoSpan('#');
    if (colType === 'currency') return monoSpan('$');
    if (colType === 'date') {
      return '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="18" rx="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/></svg>';
    }
    if (colType === 'checkbox') {
      return '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="3.5" width="17" height="17" rx="3.5"/><polyline points="8 12.5 11 15.5 16.5 9"/></svg>';
    }
    return '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h16M4 12h11M4 18h6"/><path d="M17.5 15.5l2.5 2.5 2.5-2.5"/></svg>';
  }

  $effect(() => {
    if (menuEl) menuEl.focus();
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={menuEl}
  class="sh-pop"
  class:flip-right={flipRight}
  class:is-options={view === 'options'}
  role="menu"
  tabindex="-1"
  aria-label="Column menu"
  onkeydown={handleKeydown}
  onclick={(e) => e.stopPropagation()}
  data-testid="sheet-header-menu"
>
  {#if view === 'root'}
    <button type="button" class="sh-pop__i" role="menuitem" onclick={handleRenameClick} data-testid="sheet-header-menu-rename">
      <span class="sh-pop__ico"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3.5l3.5 3.5L8 19.5H4.5V16L17 3.5z"/></svg></span>Rename<span class="sh-pop__kb">⏎</span>
    </button>
    <button type="button" class="sh-pop__i" role="menuitem" onclick={handleTypeRowClick} data-testid="sheet-header-menu-type">
      <span class="sh-pop__ico"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8h15L14 4M21 16H6l4 4"/></svg></span>Type<span class="sh-pop__kb">{typeLabel(column.col_type)}</span>
      <span class="sh-pop__chev">›</span>
    </button>
    {#if column.col_type === 'select'}
      <button type="button" class="sh-pop__i" role="menuitem" onclick={handleOptionsRowClick} data-testid="sheet-header-menu-options">
        <span class="sh-pop__ico" style="color: var(--accent)"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/></svg></span>Edit options…<span class="sh-pop__kb">{selectChoicesFromOptionsJson(column.options_json).length}</span>
        <span class="sh-pop__chev">›</span>
      </button>
    {/if}
    <button type="button" class="sh-pop__i" role="menuitem" onclick={handleInsertColumnClick} data-testid="sheet-header-menu-insert-column">
      <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="3" width="6" height="18" rx="1.6"/><path d="M16 3v18M13.5 12h5"/></svg></span>Insert column left
    </button>
    <div class="sh-pop__hr"></div>
    <button type="button" class="sh-pop__i" role="menuitem" onclick={handleSortAsc} data-testid="sheet-header-menu-sort-asc">
      <span class="sh-pop__ico"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20V4M6 10l6-6 6 6"/></svg></span>Sort ascending
    </button>
    <button type="button" class="sh-pop__i" role="menuitem" onclick={handleSortDesc} data-testid="sheet-header-menu-sort-desc">
      <span class="sh-pop__ico"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v16M6 14l6 6 6-6"/></svg></span>Sort descending
    </button>
    <button
      type="button"
      class="sh-pop__i"
      role="menuitem"
      disabled={!filterSupported(column.col_type)}
      onclick={handleFilterRowClick}
      data-testid="sheet-header-menu-filter"
    >
      <span class="sh-pop__ico"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h18l-7 8v7l-4-2.5V12L3 4z"/></svg></span>Filter…
    </button>
    <div class="sh-pop__hr"></div>
    <button type="button" class="sh-pop__i sh-pop__danger" role="menuitem" onclick={handleDeleteClick} data-testid="sheet-header-menu-delete">
      <span class="sh-pop__ico"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></svg></span>Delete column
    </button>
  {:else if view === 'type'}
    <div class="sh-pop__lab">Column type</div>
    {#each TYPES as [t, label] (t)}
      <button
        type="button"
        class="sh-pop__i"
        class:is-on={t === column.col_type}
        role="menuitemradio"
        aria-checked={t === column.col_type}
        onclick={() => handleTypeChoice(t)}
        data-testid="sheet-header-menu-type-{t}"
      >
        <span class="sh-pop__ico">{@html typeGlyphMarkup(t)}</span>{label}<span class="sh-pop__tick"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 12.5 9.5 18 20 6"/></svg></span>
      </button>
    {/each}
    <div class="sh-pop__hr"></div>
    <div class="sh-pop__sub">Values that don't parse as the new type stay as they are and render mismatched. Nothing is lost on a type change.</div>
  {:else if view === 'filter'}
    <div class="sh-pop__lab">Filter {column.name || 'column'}</div>
    {#each filterChoicesFor(column) as choice (choice)}
      {@const isOn = (filterValues ?? []).includes(choice)}
      <button
        type="button"
        class="sh-pop__i"
        class:is-on={isOn}
        role="menuitemcheckbox"
        aria-checked={isOn}
        onclick={() => toggleFilterValue(choice)}
        data-testid="sheet-header-menu-filter-value"
      >
        <span class="sh-pill">{filterLabel(column, choice)}</span>
        <span class="sh-pop__tick">✓</span>
      </button>
    {/each}
    {#if filterChoicesFor(column).length === 0}
      <div class="sh-pop__sub">No options configured for this column.</div>
    {/if}
  {:else if view === 'options'}
    <ShOptionsPanel
      choices={selectChoicesFromOptionsJson(column.options_json)}
      onChange={handleOptionsChange}
      onBack={handleOptionsBack}
    />
  {/if}
</div>

<style>
  /* #2339 PR 15b-alpha: the shared `.sh-pop*` box-model/typography rules
     now live globally in src/app.css (`.sh-pop`, `.sh-pop__lab`,
     `.sh-pop__i` + states, `.sh-pop__tick`, `.sh-pop__ico`, `.sh-pop__kb`,
     `.sh-pop__hr`, `.sh-pop__danger`, `.sh-pop__sub`) — this component only
     keeps its OWN positioning override (width + boundary-flip anchor) and
     the two classes unique to this popover (`.sh-pop__chev`, `.sh-pill`). */
  .sh-pop { width: 208px; top: 100%; left: 0; }
  .sh-pop.flip-right { left: auto; right: 0; }
  /* #2339 PR 15b-beta: the 'options' view hosts `ShOptionsPanel`, which
     draws its OWN `.sh-opt` chrome (background/border/shadow/padding at
     its own 252px width) — this root popover sheds its own chrome for
     that one view so the two don't nest into a visible double-border
     "panel in a panel". Positioning (top/left/flip-right) is unaffected. */
  .sh-pop.is-options { width: 252px; background: transparent; border: 0; box-shadow: none; padding: 0; min-width: 0; }
  .sh-pop__chev { color: var(--fg-4); display: flex; }
  .sh-pill { display: inline-flex; align-items: center; max-width: 100%; padding: 2px 8px; border-radius: var(--radius-sm); background: var(--surface-2); border: 1px solid var(--border); font-size: 11.5px; font-weight: 500; color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
