<script lang="ts">
  // SheetViewStrip.svelte — the sort/filter view strip (`ShView`, #2339
  // PR 12, §33.7, scope report
  // `docs/audits/2026-09-04-sheets-pr12-view-strip-scope.md`). A thin,
  // purely-reflective bar rendering PR 11's `sheetView.ts` state as
  // removable chips — zero engine rework, zero new backend calls, no
  // "add a sort/filter" affordance of its own (that's `ShHeaderMenu`'s
  // job). Mounted inside `SheetGrid.svelte`'s own template (scope report
  // §3, option A) — `viewState` stays owned by a single component, this
  // is just its visualization.
  //
  // Matt's rulings 2026-09-04 applied here:
  //   (1) Filter chip phrasing — exactly N-1 of N choices selected reads
  //       as exclusion ("is not X"); otherwise a positive comma-joined
  //       list ("is Lead, Contacted"). Delegated to `sheetView.ts`'s
  //       `composeFilterChipText` — this component does no phrasing logic
  //       of its own.
  //   (4) Chip order is FIXED: sort chip always renders left of the
  //       filter chip, regardless of which was set first/most recently.
  //
  // Presence-gate: renders nothing (`{#if sortColumn || filterColumn}`)
  // when neither a sort nor a filter is active — an empty, permanently-
  // present strip would train people to ignore it (design's own
  // annotation, scope report §1).
  //
  // No outside-click-to-dismiss behavior exists anywhere in this
  // component (scope report §5c) — the strip isn't a popover, it's a
  // persistent inline bar with explicit, always-visible × buttons; it
  // disappears only when its underlying view-state clears.
  import type { SheetColumn } from '$lib/types';
  import { composeSortChipText, composeFilterChipText, selectChoicesFromOptionsJson, type SortDirection } from '$lib/utils/sheetView';

  interface Props {
    sortColumn: SheetColumn | null;
    sortDirection: SortDirection | null;
    filterColumn: SheetColumn | null;
    filterValues: string[] | null;
    onClearSort: () => void;
    onClearFilter: () => void;
  }

  let { sortColumn, sortDirection, filterColumn, filterValues, onClearSort, onClearFilter }: Props = $props();

  /** The filter column's FULL configured choice list — `['0','1']` for
   *  checkbox, `options_json.choices` for select — needed by
   *  `composeFilterChipText` to detect the N-1-of-N exclusion case
   *  (scope report §1's "when computing allChoices... use
   *  selectChoicesFromOptionsJson for select columns, and ['0','1'] for
   *  checkbox columns"). */
  function allChoicesFor(column: SheetColumn): string[] {
    if (column.col_type === 'checkbox') return ['0', '1'];
    if (column.col_type === 'select') return selectChoicesFromOptionsJson(column.options_json);
    return [];
  }

  /** `composeSortChipText`/`composeFilterChipText` return the FULL
   *  sentence ("Total descending", "Status is not Cut") — the column
   *  name is a guaranteed literal prefix of that string (it's the exact
   *  same `columnName` string passed in), so slicing it + the single
   *  separating space off the front recovers just the plain-weight
   *  suffix this template renders next to the `<b>` column-name span,
   *  without duplicating the direction-word/predicate logic here. */
  let sortSuffix = $derived(
    sortColumn && sortDirection ? composeSortChipText(sortColumn.name, sortDirection).slice(sortColumn.name.length + 1) : ''
  );
  let filterSuffix = $derived(
    filterColumn && filterValues
      ? composeFilterChipText(filterColumn.name, filterValues, allChoicesFor(filterColumn), filterColumn).slice(filterColumn.name.length + 1)
      : ''
  );
</script>

{#if sortColumn || filterColumn}
  <div class="sh-view" data-testid="sheet-view-strip">
    {#if sortColumn && sortDirection}
      <span class="sh-vchip" data-testid="sheet-view-chip-sort">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          {#if sortDirection === 'asc'}
            <path d="M12 20V4M6 10l6-6 6 6" />
          {:else}
            <path d="M12 4v16M6 14l6 6 6-6" />
          {/if}
        </svg>
        <b>{sortColumn.name}</b>
        {sortSuffix}
        <button
          type="button"
          class="sh-vchip__x"
          aria-label="Clear sort"
          onclick={onClearSort}
          data-testid="sheet-view-chip-sort-clear"
        >
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />
          </svg>
        </button>
      </span>
    {/if}
    {#if filterColumn && filterValues}
      <span class="sh-vchip" data-testid="sheet-view-chip-filter">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M3 4h18l-7 8v7l-4-2.5V12L3 4z" />
        </svg>
        <b>{filterColumn.name}</b>
        {filterSuffix}
        <button
          type="button"
          class="sh-vchip__x"
          aria-label="Clear filter"
          onclick={onClearFilter}
          data-testid="sheet-view-chip-filter-clear"
        >
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />
          </svg>
        </button>
      </span>
    {/if}
    <span class="sh-view__note">View only — row order and stored data are unchanged.</span>
  </div>
{/if}

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css L88-97 — the
     "View strip" section. All 7 classes are new for PR 12; zero new
     design tokens (all of --surface-1/--surface-2/--border-faint/
     --border/--radius-pill/--accent/--fg-1/--fg-4/--overlay-press already
     exist in src/app.css). */
  .sh-view { display: flex; align-items: center; gap: 8px; padding: 7px 14px; background: var(--surface-1); border-bottom: 1px solid var(--border-faint); flex-shrink: 0; }
  .sh-vchip { display: inline-flex; align-items: center; gap: 6px; padding: 3px 4px 3px 9px; border-radius: var(--radius-pill); background: var(--surface-2); border: 1px solid var(--border); font-size: 12px; color: var(--fg-1); white-space: nowrap; }
  .sh-vchip svg { color: var(--accent); flex-shrink: 0; }
  .sh-vchip b { font-weight: 500; }
  .sh-vchip__x { width: 16px; height: 16px; border-radius: 50%; display: inline-flex; align-items: center; justify-content: center; color: var(--fg-4); background: transparent; border: 0; padding: 0; cursor: pointer; }
  .sh-vchip__x:hover { background: var(--overlay-press); color: var(--fg-1); }
  .sh-view__note { font-size: 11.5px; color: var(--fg-4); margin-left: 2px; }
</style>
