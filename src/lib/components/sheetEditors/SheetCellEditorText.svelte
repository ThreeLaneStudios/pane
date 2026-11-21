<script lang="ts">
  // SheetCellEditorText.svelte — shared plain-overlay-input editor for the
  // `text`, `number`, and `currency` column types (#2339 PR 10, §2/§7).
  // These three types share the same "flush-over-the-cell input" shape and
  // differ only in `inputmode`/alignment (`variant`) and an optional
  // trailing currency-code suffix — one component with a `variant` prop
  // rather than three near-identical files (scope report §7's own stated
  // no-strong-opinion choice).
  //
  // Commits on Enter/Tab/blur, cancels (reverts to the value the editor
  // opened with) on Escape. The caller owns the actual `update_sheet_cell`
  // API call and navigation — this component only reports intent via
  // `onCommit`/`onCancel` callback props, reading the live buffer through
  // Svelte 5's two-way `$bindable` `value` prop so a caller-driven "commit
  // because the user clicked a different cell" flow can read the
  // in-progress value without this component needing to know about clicks
  // at all.
  //
  // CSS: `.sh-ed`/`.sh-ed--num`/`.sh-ed__hint` ported verbatim from
  // `docs/design/sheets/sheets.css`. `right: -1px` is an implementation-only
  // addition (not in the design CSS, which only shows a fixed pixel width
  // in its static mockup) — needed so the editor spans the live cell's
  // actual width rather than a hardcoded value, since `.sh-c` (the parent
  // cell) is already `position: relative` and column widths vary per sheet.

  interface Props {
    value: string;
    variant: 'text' | 'number' | 'currency';
    currencyCode?: string;
    onCommit: (cause: 'enter' | 'tab' | 'shift-tab' | 'blur') => void;
    onCancel: () => void;
    /** #2874 item 12 Part D: which surface started THIS editing session
     *  — `'bar'` when `SheetGrid.svelte`'s `barBeginEdit` opened it (a
     *  click on the formula bar's idle display), `'cell'`/omitted for
     *  every other entry point (double-click, type-to-edit, ghost-row).
     *  When `'bar'`, this component skips its own `.focus()` call below
     *  so it doesn't win the mount-order race against the bar's own
     *  (Part B-guarded) focus effect — the bar's input is the one that
     *  should end up focused for a bar-initiated session. */
    origin?: 'cell' | 'bar';
  }

  let { value = $bindable(), variant, currencyCode = 'USD', onCommit, onCancel, origin = 'cell' }: Props = $props();

  let inputEl: HTMLInputElement | null = $state(null);
  let committed = false;

  $effect(() => {
    if (inputEl && origin !== 'bar') {
      inputEl.focus();
      const len = inputEl.value.length;
      inputEl.setSelectionRange(len, len);
    }
  });

  function commit(cause: 'enter' | 'tab' | 'shift-tab' | 'blur') {
    if (committed) return;
    committed = true;
    onCommit(cause);
  }

  function cancel() {
    if (committed) return;
    committed = true;
    onCancel();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      commit('enter');
    } else if (e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      commit(e.shiftKey ? 'shift-tab' : 'tab');
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    } else {
      // Every other key (including arrows, for native caret movement
      // within the input) is left alone — the grid's own keydown handler
      // never runs while an editor is open (SheetGrid.svelte gates on
      // `editingCell`), so there's no nav-vs-caret conflict to guard
      // against here.
      e.stopPropagation();
    }
  }

  function handleBlur() {
    commit('blur');
  }

  const isNumeric = $derived(variant === 'number' || variant === 'currency');
</script>

<div class="sh-ed" class:sh-ed--num={isNumeric} data-testid="sheet-cell-editor" data-variant={variant}>
  <input
    bind:this={inputEl}
    bind:value
    type="text"
    inputmode={variant === 'number' ? 'numeric' : variant === 'currency' ? 'decimal' : 'text'}
    class="sh-ed__input"
    onkeydown={handleKeydown}
    onblur={handleBlur}
    data-testid="sheet-cell-editor-input"
  />
  {#if variant === 'currency'}
    <span class="sh-cur">{currencyCode}</span>
  {/if}
  {#if variant === 'currency'}
    <span class="sh-ed__hint">Formats to {(Number(value) || 0).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} on commit</span>
  {/if}
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css. */
  .sh-ed {
    position: absolute;
    left: -1px;
    top: -1px;
    right: -1px;
    z-index: 20;
    background: var(--surface-1);
    box-shadow: inset 0 0 0 1.5px var(--accent), var(--shadow-pop);
    border-radius: 2px;
    display: flex;
    align-items: center;
    padding: 0 8px;
    box-sizing: border-box;
    color: var(--fg-1);
    font-size: 13px;
    min-height: calc(100% + 2px);
  }
  .sh-ed--num { justify-content: flex-end; font-variant-numeric: tabular-nums; }
  .sh-ed__hint {
    position: absolute;
    left: 0;
    top: 100%;
    margin-top: 4px;
    white-space: nowrap;
    font-size: 11px;
    color: var(--fg-4);
    background: var(--surface-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 3px 7px;
    z-index: 21;
  }
  .sh-cur { color: var(--fg-4); font-family: var(--font-mono); font-size: 9.5px; margin-left: 5px; letter-spacing: 0.04em; flex-shrink: 0; }

  /* Implementation-only input reset — the design's static mockup renders a
     fake caret glyph, not a real <input>; this makes a real input look
     identical (transparent background, no border, inherits editor font). */
  .sh-ed__input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    color: inherit;
    font-family: inherit;
    font-size: inherit;
    font-variant-numeric: inherit;
    padding: 0;
    text-align: inherit;
  }
  .sh-ed--num .sh-ed__input { text-align: right; }
</style>
