<script lang="ts">
  // SheetCellEditorSelect.svelte — dropdown popover editor for the `select`
  // column type (#2339 PR 10, §2/select, §7). Select is "a dropdown, not a
  // combobox" per the design annotation (sheets-grid.jsx L276): choices come
  // exclusively from the column's `options_json.choices` array (populated
  // today only via the AI tool `add_sheet_column`, per scope report §2/
  // §9.2/§10.7 — no human-authored select columns exist yet in this PR,
  // which is an accepted seam, not a bug).
  //
  // Unlike the plain-input editors, this one has no "typed buffer" — a
  // click on a choice commits immediately (matching the design's `sh-pop__i`
  // click-to-select rows), and Escape/outside-click cancels without any
  // partial value.
  //
  // #2339 PR 15b-beta: the "Edit options…" carry-forward (PR 10 scope
  // §9.2) is filled in — a footer row swaps this popover's contents to
  // `ShOptionsPanel` (same component `ShHeaderMenu.svelte` embeds for its
  // own fourth view-state; one options editor, two entry points, per
  // `docs/design/sheets/sheets-addendum.jsx`'s own "The cell dropdown's
  // existing 'Edit options…' row opens the same panel" note). `onOptions
  // Change` is OPTIONAL and defaults to a no-op so every existing caller
  // (and this file's own pre-existing tests, which never pass it) keeps
  // working unchanged — the row itself only renders when a real handler
  // is supplied.

  import ShOptionsPanel from '$lib/components/ShOptionsPanel.svelte';

  interface Props {
    value: string;
    choices: string[];
    onCommit: (choice: string) => void;
    onCancel: () => void;
    onOptionsChange?: (choices: string[]) => void;
  }

  let { value, choices, onCommit, onCancel, onOptionsChange }: Props = $props();

  type View = 'list' | 'options';
  let view: View = $state('list');

  let popEl: HTMLDivElement | null = $state(null);
  let done = false;

  function choose(choice: string) {
    if (done) return;
    done = true;
    onCommit(choice);
  }

  function cancel() {
    if (done) return;
    done = true;
    onCancel();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (view === 'options') {
        view = 'list';
      } else {
        cancel();
      }
    }
  }

  function handleWindowClick(e: MouseEvent) {
    if (e.target instanceof Node && !e.target.isConnected) return;
    if (popEl && !popEl.contains(e.target as Node)) {
      cancel();
    }
  }

  function openOptions() {
    view = 'options';
  }

  function handleOptionsChange(next: string[]) {
    onOptionsChange?.(next);
  }

  function handleOptionsBack() {
    view = 'list';
  }

  $effect(() => {
    if (popEl) {
      // Popovers open via double-click (ruling: no keyboard-open path
      // required, scope §2/select); no input to focus, but the container
      // itself takes focus so Escape/keydown work without a prior click.
      popEl.focus();
    }
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={popEl}
  class="sh-pop sh-pop--cell"
  role="listbox"
  tabindex="-1"
  aria-label="Select a value"
  onkeydown={handleKeydown}
  data-testid="sheet-cell-editor"
  data-variant="select"
>
  {#if view === 'list'}
    {#each choices as choice (choice)}
      <button
        type="button"
        class="sh-pop__i"
        class:is-on={choice === value}
        role="option"
        aria-selected={choice === value}
        onclick={() => choose(choice)}
        data-testid="sheet-select-choice"
      >
        <span class="sh-pill">{choice}</span>
        <span class="sh-pop__tick">✓</span>
      </button>
    {/each}
    {#if choices.length === 0}
      <div class="sh-pop__sub">No options configured for this column.</div>
    {/if}
    {#if onOptionsChange}
      <div class="sh-pop__hr"></div>
      <button
        type="button"
        class="sh-pop__i"
        onclick={(e) => { e.stopPropagation(); openOptions(); }}
        data-testid="sheet-select-edit-options"
      >
        <span class="sh-pop__ico">≡</span>Edit options…<span class="sh-pop__kb">{choices.length}</span>
      </button>
    {/if}
  {:else}
    <ShOptionsPanel choices={choices} onChange={handleOptionsChange} onBack={handleOptionsBack} />
  {/if}
</div>

<style>
  /* #2339 PR 15b-alpha: shared `.sh-pop*` box-model/typography rules now
     live globally in src/app.css — this component keeps only its own
     positioning override (`.sh-pop--cell`) and `.sh-pill`, which isn't
     part of the shared shell. */
  .sh-pop--cell { top: calc(100% + 2px); left: -1px; }
  .sh-pill { display: inline-flex; align-items: center; max-width: 100%; padding: 2px 8px; border-radius: var(--radius-sm); background: var(--surface-2); border: 1px solid var(--border); font-size: 11.5px; font-weight: 500; color: var(--fg-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
