<script lang="ts">
  // ShOptionsPanel.svelte — the select-column choices editor (#2339 PR
  // 15b-beta, `docs/design/sheets/sheets-addendum.jsx:133-260` OptRow/
  // OptPanel, `NOTES.md` §B). Reached two ways — `ShHeaderMenu.svelte`'s
  // "Edit options…" row (select columns only) and
  // `SheetCellEditorSelect.svelte`'s own "Edit options…" footer row —
  // both mount THIS one component rather than duplicating the panel.
  //
  // Six states, all in this one file (per the design's single `OptPanel`
  // with a `state` prop, translated here into real interaction rather
  // than a static illustration):
  //   list      — the default: every choice as a chip + hover-✕ row
  //   add       — a focused empty input appended at the end, Enter commits
  //   rename    — one row's chip swapped for an in-place input
  //   dupe      — add/rename's input takes the danger ring + shake +
  //               a one-line collision message (this is a MODIFIER on
  //               add/rename, not a fifth mode — see `errorMessage` below)
  //   reorder   — dragging a row's grip shows a cyan drop line
  //   empty     — `choices.length === 0`: "No choices yet" + one button
  //
  // Persistence: this component NEVER calls `updateSheetColumn` itself —
  // it's a pure "here's the new choices array" emitter via `onChange`,
  // exactly like `ShHeaderMenu`'s own `onTypeChange`/`onRename` callback
  // shape. The caller (`SheetGrid.svelte`) owns the actual
  // `api.updateSheetColumn({ optionsJson })` call + refetch, mirroring
  // its existing `menuTypeChange` handler. This keeps every array
  // mutation in this file testable via plain `onChange` spy assertions,
  // with zero network/Tauri mocking needed in this component's own tests.
  //
  // Removal safety (NOTES.md §B, ratified): removing a choice ONLY ever
  // produces a shorter `choices` array via `removeChoiceAt` — there is no
  // code path here that touches `sheet_cells` at all. A cell holding the
  // removed choice starts rendering as a mismatch purely because
  // `isTypeMismatch` (sheetView.ts) is re-evaluated against the new list
  // on the next grid render — see `sheetOptions.test.ts`'s dedicated test
  // for this exact claim, exercised at the pure-function level.

  import {
    isDuplicateChoice,
    duplicateChoiceMessage,
    addChoice,
    renameChoiceAt,
    removeChoiceAt,
    reorderChoicesByRawDrop,
  } from '$lib/utils/sheetOptions';

  interface Props {
    /** The column's current choices, in `options_json.choices` order —
     *  the SAME order the cell dropdown shows (NOTES.md §B: "There is no
     *  alphabetical mode"). */
    choices: string[];
    /** Fired with the FULL new choices array after every committed
     *  add/rename/remove/reorder — see persistence note above. */
    onChange: (choices: string[]) => void;
    /** Back-chevron → root header menu (only meaningful when this panel
     *  is embedded inside `ShHeaderMenu`; `SheetCellEditorSelect`'s
     *  footer entry point passes a callback that closes its own popover
     *  instead). */
    onBack: () => void;
  }

  let { choices, onChange, onBack }: Props = $props();

  type Mode = 'list' | 'add' | 'rename';
  let mode: Mode = $state('list');
  let renameIndex: number = $state(-1);
  let draftValue: string = $state('');
  let errorMessage: string | null = $state(null);
  /** Bumped on every rejected commit so `sh-shake`'s animation restarts
   *  even when the SAME duplicate is retried twice in a row (a CSS class
   *  toggle alone wouldn't retrigger the keyframe on an unchanged
   *  class list — `key`-ing the shake off this nonce, via the
   *  `data-shake` attribute below, forces it). */
  let shakeNonce = $state(0);

  let dragIndex: number | null = $state(null);
  /** The index the dragged row would be inserted BEFORE if dropped now
   *  (raw, pre-removal index — `reorderChoicesByRawDrop`'s own
   *  contract). `null` when nothing is being dragged over. */
  let dropIndex: number | null = $state(null);

  let addInputEl: HTMLInputElement | null = $state(null);
  let renameInputEl: HTMLInputElement | null = $state(null);

  $effect(() => {
    if (mode === 'add' && addInputEl) addInputEl.focus();
  });
  $effect(() => {
    if (mode === 'rename' && renameInputEl) {
      renameInputEl.focus();
      renameInputEl.select();
    }
  });

  function clearError() {
    errorMessage = null;
  }

  function startAdd() {
    draftValue = '';
    clearError();
    mode = 'add';
  }

  function startRename(index: number) {
    draftValue = choices[index];
    renameIndex = index;
    clearError();
    mode = 'rename';
  }

  function cancelEdit() {
    mode = 'list';
    draftValue = '';
    renameIndex = -1;
    clearError();
  }

  function rejectDuplicate(candidate: string) {
    errorMessage = duplicateChoiceMessage(candidate);
    shakeNonce++;
  }

  function commitAdd() {
    const value = draftValue.trim();
    if (!value) {
      cancelEdit();
      return;
    }
    if (isDuplicateChoice(choices, value)) {
      rejectDuplicate(value);
      return;
    }
    onChange(addChoice(choices, value));
    cancelEdit();
  }

  function commitRename() {
    const value = draftValue.trim();
    if (!value) {
      cancelEdit();
      return;
    }
    if (isDuplicateChoice(choices, value, renameIndex)) {
      rejectDuplicate(value);
      return;
    }
    onChange(renameChoiceAt(choices, renameIndex, value));
    cancelEdit();
  }

  function commitEdit() {
    if (mode === 'add') commitAdd();
    else if (mode === 'rename') commitRename();
  }

  function handleEditKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitEdit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelEdit();
    }
  }

  function handleEditBlur() {
    // A blur while an error is showing means focus already left (jsdom/
    // real browsers both fire blur on Escape-driven `.blur()` too) — in
    // that case `cancelEdit` already ran via the keydown handler, so this
    // is a no-op guard against double-handling, not a real second path.
    if (mode === 'list') return;
    commitEdit();
  }

  function removeChoice(index: number) {
    onChange(removeChoiceAt(choices, index));
  }

  function handleDragStart(index: number) {
    dragIndex = index;
  }

  function handleDragOverRow(e: DragEvent, index: number) {
    e.preventDefault();
    dropIndex = index;
  }

  function handleDragEnd() {
    dragIndex = null;
    dropIndex = null;
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    if (dragIndex !== null && dropIndex !== null) {
      onChange(reorderChoicesByRawDrop(choices, dragIndex, dropIndex));
    }
    dragIndex = null;
    dropIndex = null;
  }
</script>

<div class="sh-opt" data-testid="sheet-options-panel">
  <div class="sh-opt__head">
    <button
      type="button"
      class="sh-opt__back"
      aria-label="Back"
      onclick={onBack}
      data-testid="sheet-options-back"
    ><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg></button>
    <span class="sh-opt__t">Options</span>
    <span class="sh-opt__n" data-testid="sheet-options-count">{choices.length}</span>
  </div>

  {#if choices.length === 0 && mode === 'list'}
    <div class="sh-opt__empty" data-testid="sheet-options-empty">
      <b>No choices yet.</b><br />Cells in this column are empty until you add one. Existing values stay as they are and render as mismatches.
    </div>
    <button type="button" class="sh-opt__add" onclick={startAdd} data-testid="sheet-options-add-first">
      <span class="sh-pop__ico"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14 M5 12h14"/></svg></span>Add the first choice
    </button>
  {:else}
    <div class="sh-opt__list" data-testid="sheet-options-list">
      {#each choices as choice, i (i)}
        {#if dropIndex === i && dragIndex !== null && dragIndex !== i}
          <div class="sh-opt__drop" data-testid="sheet-options-drop-line"></div>
        {/if}
        <div
          class="sh-opt__i"
          class:is-drag={dragIndex === i}
          draggable={mode === 'list'}
          ondragstart={() => handleDragStart(i)}
          ondragover={(e) => handleDragOverRow(e, i)}
          ondragend={handleDragEnd}
          ondrop={handleDrop}
          data-testid="sheet-options-row-{i}"
        >
          <span class="sh-opt__grip" aria-hidden="true"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6h.01M9 12h.01M9 18h.01M15 6h.01M15 12h.01M15 18h.01" stroke-width="2.6"/></svg></span>
          <span class="sh-opt__val">
            {#if mode === 'rename' && renameIndex === i}
              <input
                bind:this={renameInputEl}
                bind:value={draftValue}
                class="sh-opt__in"
                class:is-error={errorMessage !== null}
                data-shake={shakeNonce}
                onkeydown={handleEditKeydown}
                onblur={handleEditBlur}
                data-testid="sheet-options-rename-input"
              />
            {:else}
              <span class="sh-chipfake" ondblclick={() => startRename(i)}>{choice}</span>
            {/if}
          </span>
          {#if !(mode === 'rename' && renameIndex === i)}
            <button
              type="button"
              class="sh-opt__x"
              aria-label="Remove {choice}"
              onclick={() => removeChoice(i)}
              data-testid="sheet-options-remove-{i}"
            ><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/></svg></button>
          {/if}
        </div>
        {#if mode === 'rename' && renameIndex === i && errorMessage}
          <div class="sh-opt__err" data-testid="sheet-options-error"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.5l9.5 16.5H2.5L12 3.5zM12 9.5v4.5M12 17.4v.1"/></svg><span>{errorMessage}</span></div>
        {/if}
      {/each}
      {#if dropIndex === choices.length && dragIndex !== null}
        <div class="sh-opt__drop" data-testid="sheet-options-drop-line"></div>
      {/if}
    </div>

    {#if mode === 'add'}
      <div class="sh-opt__i">
        <span class="sh-opt__grip"></span>
        <span class="sh-opt__val">
          <input
            bind:this={addInputEl}
            bind:value={draftValue}
            class="sh-opt__in"
            class:is-error={errorMessage !== null}
            data-shake={shakeNonce}
            placeholder="Choice name"
            onkeydown={handleEditKeydown}
            onblur={handleEditBlur}
            data-testid="sheet-options-add-input"
          />
        </span>
      </div>
      {#if errorMessage}
        <div class="sh-opt__err" data-testid="sheet-options-error"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.5l9.5 16.5H2.5L12 3.5zM12 9.5v4.5M12 17.4v.1"/></svg><span>{errorMessage}</span></div>
      {/if}
    {:else}
      <button type="button" class="sh-opt__add" onclick={startAdd} data-testid="sheet-options-add">
        <span class="sh-pop__ico"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14 M5 12h14"/></svg></span>Add choice
      </button>
    {/if}

    <div class="sh-opt__foot">Removing a choice leaves cell values untouched. A cell holding a removed choice renders as a mismatch.</div>
  {/if}
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css (`.sh-opt*`
     block + `@keyframes sh-shake`). */
  .sh-opt { width: 252px; box-sizing: border-box; background: #1c1c20; border: 1px solid var(--border-strong); border-radius: 10px; box-shadow: var(--shadow-pop); padding: 5px; }
  .sh-opt__head { display: flex; align-items: center; gap: 7px; padding: 7px 8px 6px; }
  .sh-opt__back { width: 20px; height: 20px; border: 0; background: transparent; border-radius: var(--radius-sm); display: flex; align-items: center; justify-content: center; color: var(--fg-3); flex-shrink: 0; cursor: pointer; }
  .sh-opt__back:hover { background: var(--overlay-hover); color: var(--fg-1); }
  .sh-opt__t { font-size: 12.5px; font-weight: 600; color: var(--fg-1); flex: 1; min-width: 0; }
  .sh-opt__n { font-family: var(--font-mono); font-size: 10px; color: var(--fg-4); }
  .sh-opt__list { padding: 2px 0; }
  .sh-opt__i { display: flex; align-items: center; gap: 6px; padding: 4px 6px 4px 4px; border-radius: var(--radius-sm); }
  .sh-opt__i:hover { background: var(--overlay-hover); }
  .sh-opt__grip { width: 14px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: var(--fg-4); cursor: grab; }
  .sh-opt__i:hover .sh-opt__grip { color: var(--fg-3); }
  .sh-opt__val { flex: 1; min-width: 0; }
  .sh-opt__x { width: 20px; height: 20px; border: 0; background: transparent; border-radius: var(--radius-sm); display: flex; align-items: center; justify-content: center; color: var(--fg-4); flex-shrink: 0; opacity: 0; cursor: pointer; }
  .sh-opt__i:hover .sh-opt__x { opacity: 1; }
  .sh-opt__x:hover { background: rgba(239, 68, 68, 0.13); color: var(--status-danger); }
  .sh-opt__i.is-drag { background: var(--surface-2); box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45); }
  .sh-opt__i.is-drag .sh-opt__grip { color: var(--accent); }
  .sh-opt__drop { height: 2px; background: var(--accent); border-radius: 1px; margin: 1px 6px; }
  .sh-opt__add { display: flex; align-items: center; gap: 7px; width: 100%; padding: 7px 8px; margin-top: 2px; border: 0; background: transparent; border-radius: var(--radius-sm); color: var(--fg-3); font-family: var(--font-sans); font-size: 12.5px; text-align: left; cursor: pointer; }
  .sh-opt__add:hover { background: var(--overlay-hover); color: var(--fg-1); }
  .sh-opt__add .sh-pop__ico { color: var(--accent); }
  .sh-opt__in { width: 100%; box-sizing: border-box; background: var(--surface-2); border: 1px solid var(--accent); border-radius: var(--radius-sm); padding: 5px 8px; color: var(--fg-1); font-family: var(--font-sans); font-size: 12.5px; }
  .sh-opt__in.is-error { border-color: var(--status-danger); box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.18); animation: sh-shake 240ms var(--ease); }
  @keyframes sh-shake { 0%, 100% { transform: translateX(0); } 20% { transform: translateX(-4px); } 45% { transform: translateX(3px); } 70% { transform: translateX(-2px); } }
  .sh-opt__err { display: flex; align-items: flex-start; gap: 6px; padding: 2px 8px 6px 24px; font-size: 11.5px; line-height: 1.4; color: var(--status-danger); }
  .sh-opt__foot { padding: 7px 9px 5px; border-top: 1px solid var(--border-faint); margin-top: 4px; font-size: 11.5px; color: var(--fg-4); line-height: 1.45; }
  .sh-opt__empty { padding: 4px 9px 10px; font-size: 12px; color: var(--fg-4); line-height: 1.5; }
  .sh-opt__empty b { color: var(--fg-2); font-weight: 500; }
  .sh-chipfake { display: inline-flex; align-items: center; max-width: 100%; padding: 2px 8px; border-radius: var(--radius-sm); background: var(--surface-2); border: 1px solid var(--border); font-size: 11.5px; font-weight: 500; color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sh-pop__ico { width: 16px; flex-shrink: 0; display: flex; justify-content: center; color: var(--fg-3); }
</style>
