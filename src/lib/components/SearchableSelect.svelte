<script lang="ts">
  /**
   * SearchableSelect.svelte — #3171 PR 2, C51/C57.
   *
   * Generic extraction of SearchableModelSelect.svelte's core mechanics
   * (search-filter input, fixed-position dropdown, open/close-on-outside-
   * click/scroll/resize, selection) so the SAME component backs the
   * image-model picker (Settings.svelte, §39.2), the New-job dialog's
   * Project field, and its Agent field (ScheduledPanel.svelte) — Matt's
   * explicit ruling ("it has to be the same component the model pickers
   * use. One fix for all three.").
   *
   * This file owns ONLY the item-agnostic mechanics: text input state,
   * dropdown open/close, fixed-positioning math, filter/sort delegation,
   * and the selected/hover row chrome (the `<button class="dropdown-item">`
   * wrapper itself, incl. `class:selected`). It does NOT know what an item
   * "is" — callers supply `filterItem`/`sortItems` (the comparison logic)
   * and a `renderItem` snippet (the row's inner content: label, tags,
   * whatever). This mirrors exactly how SearchableModelSelect.svelte
   * still owns model-only concerns (capability tags, the custom-model
   * entry box, price display) by wrapping this component rather than
   * duplicating its mechanics — see that file's own header comment.
   *
   * An item is any object with a stable `id: string`. `getDisplayText`
   * resolves the closed-state input value from an id — for a list-backed
   * selection it's normally the item's own label; for a free-typed value
   * not present in `items` (SearchableModelSelect's "custom model" case)
   * the caller's own fallback decides what to show.
   */
  import { onMount, type Snippet } from 'svelte';

  interface SearchableSelectItem {
    id: string;
  }

  interface Props {
    items: SearchableSelectItem[];
    selectedId?: string;
    placeholder?: string;
    onSelect: (id: string) => void;
    // Resolves the closed-state input value for the current `selectedId`
    // (including the empty-string case — callers return '' themselves).
    getDisplayText: (id: string) => string;
    // Only consulted when `searchText` is non-empty — mirrors
    // SearchableModelSelect's original `filteredModels` derivation
    // exactly (no filtering at all while the box is empty).
    filterItem: (item: SearchableSelectItem, lowerSearch: string) => boolean;
    // Applied AFTER filtering (or to the full list when searchText is
    // empty) — same two-step shape as the original `filteredModels`.
    // #3178 (C57 ordering): the second argument is the SAME lowercased
    // query `filterItem`'s own second argument receives ('' when the
    // search box is empty) — added so a caller whose list pins one item
    // regardless of query (`ScheduledPanel`'s Project field's "Home")
    // can still rank an actual typed match ahead of it, which requires
    // knowing what was typed. Existing callers that ignore the query
    // entirely (`formItemsUnsorted`, `SearchableModelSelect`'s own
    // provider/name sort) keep working unchanged — a function declared
    // with fewer parameters is assignable wherever more are expected.
    sortItems: (items: SearchableSelectItem[], query: string) => SearchableSelectItem[];
    // Row inner content. Receives the live `searchText` so a caller can
    // highlight the match the same way SearchableModelSelect's
    // `highlightText()` call always has.
    renderItem: Snippet<[SearchableSelectItem, string]>;
    // Appended inside the dropdown, after the filtered list — e.g.
    // SearchableModelSelect's "Enter custom model name..." action.
    // Receives a `closeDropdown` callback so a footer action that
    // completes its own selection (via `onSelect` directly) can also
    // close/reset this component's open+search state, matching what
    // `selectModel()` below already does for a normal row click.
    footer?: Snippet<[() => void]>;
    // Extra classes merged onto the two outer elements so existing,
    // already-tested call sites (SearchableModelSelect's
    // `.searchable-model-select`/`.model-input`) keep their own
    // selectors working unchanged — see that file's wrapper usage.
    containerClass?: string;
    inputClass?: string;
    testId?: string;
    // #3171 PR 2: ScheduledPanel's Agent field locks while editing a job
    // (§36.3.3 — the session is permanently bound to its agent). Forwarded
    // straight to the underlying <input disabled>, same gate the native
    // `<select disabled={...}>` it replaces already had.
    disabled?: boolean;
    // #3178 (C57, rewrite, Matt's ruling): default `false` — floating
    // (fixed-position, viewport-anchored) is unchanged, existing callers
    // (Settings' image-model picker, SearchableModelSelect) are
    // unaffected by this prop's mere existence.
    //
    // C57's history is five passes over this file, four of them pure
    // measurement fixes, each correct for the case that prompted it and
    // wrong for the next: #3176 fixed the containing-block bug and the
    // list anchored over the preset cards; the #3178/#3180 threshold
    // change (MIN_USABLE_SPACE_BELOW) fixed the overlap and left 72-150px
    // of the list hanging off the bottom of the window; the height-clamp
    // follow-up (reverted immediately before this prop was added — see
    // that revert commit) fixed the overflow and introduced a stale
    // `actualHeight` that corrupted a later open of the SAME instance
    // (a clamp that never grows back on resize; a clamp whose *size*
    // reset on an 'above' transition but whose *position* didn't,
    // overlapping the field by ~220px). That is the C27 shape repeating:
    // each fix patches the symptom of "how much screen-fixed space is
    // there, right now, in every direction" without ever being able to
    // prove it handles the NEXT direction-of-change (resize, reopen,
    // above-then-below-then-above) too — because the measurement that
    // question needs (the viewport, a moving target) is the thing
    // changing out from under it between opens.
    //
    // This prop is the structural fix, not a sixth measurement: in
    // inline mode there is no viewport-relative math to get wrong at
    // all. The list renders in normal document flow directly under its
    // field (`position: static`, no `top`/`left`/`width`/`max-height`
    // coordinates ever computed) and pushes the page's own content
    // down; whatever scrolls the page scrolls the list. There is no
    // `dropdownTop`, no clamp, no `actualHeight` read, and no
    // above/below branch for an inline instance to get stale — see
    // `updateDropdownPosition`'s own guard clause below. Do not
    // "simplify" this back to one mode: floating mode still exists,
    // correctly, for pickers with genuine screen-fixed-overlay needs
    // (the two AI-tab model pickers, which "have room and have never
    // failed" — Matt) — the two modes are deliberately different, not
    // a leftover migration step. See docs/spec/runtime.md §36.3.8 for
    // the recorded rationale and docs/spec entry listing both modes.
    inline?: boolean;
  }

  let {
    items,
    selectedId = '',
    placeholder = 'Search...',
    onSelect,
    getDisplayText,
    filterItem,
    sortItems,
    renderItem,
    footer,
    containerClass = '',
    inputClass = '',
    testId,
    disabled = false,
    inline = false,
  }: Props = $props();

  let searchText = $state('');
  let isOpen = $state(false);
  let containerRef: HTMLDivElement | null = $state(null);
  let inputRef: HTMLInputElement | null = $state(null);
  let dropdownRef: HTMLDivElement | null = $state(null);

  let displayText = $derived(() => getDisplayText(selectedId));

  // Fixed positioning coordinates — identical math to
  // SearchableModelSelect's original `updateDropdownPosition`.
  let dropdownTop = $state(0);
  let dropdownLeft = $state(0);
  let dropdownWidth = $state(0);
  let dropdownPosition = $state<'below' | 'above'>('below');

  // #3178 (C57, third attempt): below this many px of room, "below"
  // stops being usable even with its own `overflow-y: auto` scroll — a
  // `.dropdown-item` row is ~36-40px (10px vertical padding + a 14px
  // line + its 1px border-bottom; see `.dropdown-item`'s own rule
  // below), so 120px is roughly 3 visible rows: enough to recognize
  // you're in the right list and scroll for more, not a sliver.
  const MIN_USABLE_SPACE_BELOW = 120;

  function updateDropdownPosition() {
    // #3178 (C57, rewrite): inline instances never reach this function
    // (both call sites below already skip it), but the guard stays as
    // its own defense-in-depth line, not just a comment — an inline
    // instance must never compute `dropdownTop`/`dropdownMaxHeight`/
    // `actualHeight` even if a future caller adds a third call site
    // that forgets the `!inline` check.
    if (inline || !containerRef) return;
    const rect = containerRef.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;

    const actualHeight = dropdownRef ? dropdownRef.getBoundingClientRect().height : 300;

    dropdownLeft = rect.left;
    dropdownWidth = rect.width;

    // #3178 (C57, third attempt): this used to fire "above" whenever
    // there was merely MORE raw viewport space above the field than
    // below it (`spaceAbove > spaceBelow`), even when `spaceBelow` was
    // already a perfectly usable amount on its own (a few rows visible,
    // scroll for the rest — `.dropdown-list`'s `overflow-y: auto` below
    // already supports exactly that). That's wrong whenever the field
    // sits deep inside a scrollable form with real sibling content
    // above it (the New job dialog's Project field, a long project
    // list, a modest window): "above" reached all the way up past the
    // field's own nearby siblings into the dialog's header/preset-cards
    // zone — Reed's live measurement (#3178): computed `top=227px`
    // against the field's true `top=527px`, squarely inside that zone.
    // "Above" is now reserved for when space below is genuinely too
    // small to be useful at all — matching the product expectation
    // directly ("directly below the field, above only when there
    // genuinely isn't room"). The two AI-tab model pickers this
    // component also backs are unaffected: their `spaceBelow` is large
    // enough that neither the old nor the new condition ever picks
    // "above" for them.
    if (spaceBelow < MIN_USABLE_SPACE_BELOW && spaceAbove > spaceBelow) {
      dropdownPosition = 'above';
      dropdownTop = Math.max(8, rect.top - Math.min(actualHeight, spaceAbove - 8));
    } else {
      dropdownPosition = 'below';
      dropdownTop = rect.bottom + 4;
    }
  }

  let filteredItems = $derived(() => {
    if (!searchText) {
      return sortItems([...items], '');
    }
    const lowerSearch = searchText.toLowerCase();
    return sortItems(items.filter((item) => filterItem(item, lowerSearch)), lowerSearch);
  });

  function handleFocus() {
    if (disabled) return;
    isOpen = true;
    searchText = '';
    // #3178 (C57, rewrite): inline instances skip the positioning pass
    // entirely — there is no fixed-position box to place, so there is
    // nothing for `updateDropdownPosition` to compute.
    if (!inline) {
      requestAnimationFrame(() => {
        updateDropdownPosition();
        requestAnimationFrame(() => updateDropdownPosition());
      });
    }
  }

  function handleBlur(e: FocusEvent) {
    setTimeout(() => {
      if (containerRef && !containerRef.contains(e.relatedTarget as Node)) {
        isOpen = false;
        searchText = '';
      }
    }, 200);
  }

  function closeDropdown() {
    isOpen = false;
    searchText = '';
    inputRef?.blur();
  }

  function selectItem(id: string) {
    onSelect(id);
    closeDropdown();
  }

  onMount(() => {
    function handleClickOutside(e: MouseEvent) {
      if (
        containerRef &&
        !containerRef.contains(e.target as Node) &&
        dropdownRef &&
        !dropdownRef.contains(e.target as Node)
      ) {
        isOpen = false;
        searchText = '';
      }
    }

    function handleScrollOrResize() {
      // #3178 (C57, rewrite): an inline instance has no fixed-position
      // box to re-place on scroll/resize — it's a reset avoided before
      // the last rewrite's stale-`actualHeight` bug (§ this prop's own
      // doc comment, above) ever gets the chance to exist for it.
      if (isOpen && !inline) {
        updateDropdownPosition();
      }
    }

    document.addEventListener('mousedown', handleClickOutside);
    window.addEventListener('scroll', handleScrollOrResize, true);
    window.addEventListener('resize', handleScrollOrResize);

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('scroll', handleScrollOrResize, true);
      window.removeEventListener('resize', handleScrollOrResize);
    };
  });
</script>

<div bind:this={containerRef} class="searchable-select {containerClass}">
  <input
    bind:this={inputRef}
    type="text"
    class="select-input-field {inputClass}"
    value={isOpen ? searchText : displayText()}
    placeholder={placeholder}
    title={displayText()}
    data-testid={testId}
    disabled={disabled}
    onfocus={handleFocus}
    onblur={handleBlur}
    oninput={(e) => (searchText = e.currentTarget.value)}
  />
  <div class="dropdown-arrow" class:open={isOpen}>
    <svg width="12" height="8" viewBox="0 0 12 8" fill="none">
      <path d="M1 1L6 6L11 1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>
  </div>

  {#snippet dropdownItems()}
    {#each filteredItems() as item (item.id)}
      <button
        type="button"
        class="dropdown-item"
        class:selected={item.id === selectedId}
        onmousedown={(e) => {
          e.preventDefault();
          selectItem(item.id);
        }}
      >
        {@render renderItem(item, searchText)}
      </button>
    {/each}

    {#if footer}
      {@render footer(closeDropdown)}
    {/if}
  {/snippet}

  {#if isOpen}
    {#if inline}
      <!-- #3178 (C57, rewrite): in flow, directly under the field — no
           fixed positioning, no top/left/width coordinates computed at
           all. The dialog's own scroll reaches the last item; there is
           no inner scroll box to miss it. See the `inline` prop's own
           doc comment above for why this mode exists. -->
      <div bind:this={dropdownRef} class="dropdown-list dropdown-list-inline">
        {@render dropdownItems()}
      </div>
    {:else}
      <div
        bind:this={dropdownRef}
        class="dropdown-list"
        data-position={dropdownPosition}
        style:top="{dropdownTop}px"
        style:left="{dropdownLeft}px"
        style:width="{dropdownWidth}px"
      >
        {@render dropdownItems()}
      </div>
    {/if}
  {/if}
</div>

<style>
  .searchable-select {
    position: relative;
    width: 100%;
  }

  .select-input-field {
    width: 100%;
    background: #2C2C2E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    padding: 10px 32px 10px 12px;
    font-size: 14px;
    color: #F0EDE8;
    font-family: inherit;
    transition: border-color 0.2s;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .select-input-field:focus {
    outline: none;
    border-color: #00E4E4;
  }

  .select-input-field::placeholder {
    color: #666666;
  }

  .select-input-field:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dropdown-arrow {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    color: rgba(255, 255, 255, 0.4);
    pointer-events: none;
    transition: transform 0.2s;
  }

  .dropdown-arrow.open {
    transform: translateY(-50%) rotate(180deg);
  }

  .dropdown-list {
    position: fixed;
    max-height: 300px;
    background: #2C2C2E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    overflow-y: auto;
    z-index: 9999;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  }

  /* #3178 (C57, rewrite): inline mode renders in normal document flow
     under its field instead of as a viewport-anchored overlay — no
     `position: fixed`, no coordinates, no own scroll container. The
     list's own height grows with its content and pushes whatever sits
     below it in the dialog down; the dialog's own scroll (not this
     element's `overflow-y`) is what reaches the last item. */
  .dropdown-list-inline {
    position: static;
    width: 100%;
    /* #3182 C64 sibling: capped to ~5 rows, same calc() shape as
       Card.svelte's `calc(11px * 1.4 * 3)` precedent. `.dropdown-item`
       is font-size: 14px with no explicit line-height (previously
       relying on browser "normal" ~1.15-1.2x, non-deterministic) —
       pinned here to 1.4 so the row height is an auditable number:
       14px * 1.4 (text line) + 10px + 10px (vertical padding) + 1px
       (border-bottom) = 40.6px/row * 5 rows ~= 203px. Was `none` with
       `overflow-y: visible` (#3178 C57) — that was correct for C57's
       no-measurement rule but let the list (and the dialog around it)
       grow to fit the whole Agent list and run off the bottom. Still
       CSS-only, still no position/measurement — just a cap + scroll. */
    max-height: calc((14px * 1.4 + 10px + 10px + 1px) * 5);
    overflow-y: auto;
    box-shadow: none;
    margin-top: 4px;
  }

  .dropdown-list::-webkit-scrollbar {
    width: 6px;
  }

  .dropdown-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .dropdown-list::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.1);
    border-radius: 3px;
  }

  .dropdown-list::-webkit-scrollbar-thumb:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  .dropdown-item {
    width: 100%;
    text-align: left;
    padding: 10px 12px;
    font-size: 14px;
    background: none;
    border: none;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    cursor: pointer;
    transition: background 0.15s;
    color: inherit;
  }

  .dropdown-item:last-child {
    border-bottom: none;
  }

  .dropdown-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .dropdown-item.selected {
    background: rgba(0, 228, 228, 0.1);
  }
</style>
