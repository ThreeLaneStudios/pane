<script lang="ts">
  // SheetCellEditorFormula.svelte — in-cell formula-editing variant
  // (#2339 PR 14, §33.5). Opens whenever a cell's live edit buffer starts
  // with `=` — see SheetGrid.svelte's template branch ordering (checked
  // BEFORE the `date`/generic-text branches, so ANY typed column except
  // checkbox/select can enter formula mode; checkbox has no editor at
  // all and select is checked first in the same template, satisfying
  // Matt ruling 2026-09-05 #2 — no `=` escape hatch for those two types).
  //
  // Same "flush-over-the-cell input" shape as SheetCellEditorText.svelte,
  // but layers a colored token overlay UNDER a caret-only-visible input
  // (the input's own text is transparent; the overlay renders the same
  // string through `tokenizeFormula` with the bar's exact `.sh-t-*`
  // classes) — the standard "highlight-behind-transparent-input" technique
  // for live-colored-while-typing text inputs, giving a REAL synced caret
  // rather than a static color preview. This is the SAME buffer
  // (`bind:value`) the formula bar reads/writes via SheetGrid's
  // `barUpdateValue` export — "one formula, two places to edit it."
  import { tokenizeFormula } from '$lib/utils/sheetFormula';

  interface Props {
    value: string;
    onCommit: (cause: 'enter' | 'tab' | 'shift-tab' | 'blur') => void;
    onCancel: () => void;
    /** #2339 PR 14 fix (bar/cell caret parity): the shared editing
     *  session's live caret position, mirroring `FormulaBar.svelte`'s own
     *  `selectionStart`/`selectionEnd` props — the SAME `editing` session
     *  object on `SheetGrid.svelte` backs both, so whichever surface the
     *  edit last had focus in leaves its caret here for the other surface
     *  to pick up on focus. `null`/absent falls back to end-of-text,
     *  matching this component's original always-end-of-text behavior. */
    selectionStart?: number | null;
    selectionEnd?: number | null;
    /** Fired on every caret/selection change — the other half of the
     *  `selectionStart`/`selectionEnd` props above. */
    onSelectionChange?: (start: number, end: number) => void;
    /** #2874 item 12 Part D: which surface started THIS editing session
     *  — see `SheetCellEditorText.svelte`'s identical doc comment. When
     *  `'bar'`, skips this component's own `.focus()` call so the bar's
     *  own input wins the mount-order race instead. */
    origin?: 'cell' | 'bar';
  }

  let {
    value = $bindable(),
    onCommit,
    onCancel,
    selectionStart = null,
    selectionEnd = null,
    onSelectionChange,
    origin = 'cell',
  }: Props = $props();

  let inputEl: HTMLInputElement | null = $state(null);
  let committed = false;

  $effect(() => {
    if (inputEl && origin !== 'bar') {
      inputEl.focus();
      const len = inputEl.value.length;
      const start = selectionStart ?? len;
      const end = selectionEnd ?? len;
      inputEl.setSelectionRange(Math.min(start, len), Math.min(end, len));
    }
  });

  /** Reports the input's current caret/selection up to the parent so it
   *  can carry it over if focus moves to the formula bar next. */
  function reportSelection() {
    if (!inputEl) return;
    onSelectionChange?.(inputEl.selectionStart ?? 0, inputEl.selectionEnd ?? 0);
  }

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
      e.stopPropagation();
    }
  }

  function handleBlur() {
    commit('blur');
  }

  let tokens = $derived(tokenizeFormula(value ?? ''));
</script>

<div class="sh-ed sh-ed--formula" data-testid="sheet-cell-editor" data-variant="formula">
  <div class="sh-ed__hl" aria-hidden="true" data-testid="sheet-cell-editor-formula-highlight">
    {#each tokens as tok, i (i)}<span class={tok.cls}>{tok.text}</span>{/each}
  </div>
  <input
    bind:this={inputEl}
    bind:value
    type="text"
    class="sh-ed__input sh-ed__input--formula"
    onkeydown={handleKeydown}
    onblur={handleBlur}
    oninput={reportSelection}
    onclick={reportSelection}
    onkeyup={reportSelection}
    onselect={reportSelection}
    data-testid="sheet-cell-editor-input"
  />
</div>

<style>
  /* Base shell ported verbatim from SheetCellEditorText.svelte /
     docs/design/sheets/sheets.css. */
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
  .sh-ed--formula { font-family: var(--font-mono); font-size: 12.5px; }

  /* Implementation-only highlight-behind-transparent-input technique: the
     overlay and the input share identical box metrics (font, padding,
     line-height) and are stacked via `position: absolute`/relative parent,
     so the colored spans sit exactly under the typed characters. Only the
     REAL input receives keystrokes/caret/selection — the overlay is
     `pointer-events: none` and `aria-hidden`, purely visual. */
  .sh-ed__hl {
    position: absolute;
    left: 8px;
    right: 8px;
    top: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    white-space: pre;
    overflow: hidden;
    pointer-events: none;
    font-family: inherit;
    font-size: inherit;
  }
  .sh-ed__input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    font-family: inherit;
    font-size: inherit;
    padding: 0;
    text-align: inherit;
    position: relative;
    z-index: 1;
    /* The real input's own glyphs are invisible — only its caret shows —
       so the colored overlay underneath is what the user actually reads.
       The caret itself stays the accent color, matching `.sh-caret`. */
    color: transparent;
    caret-color: var(--accent);
  }

  :global(.sh-ed__hl .sh-t-fn) { color: var(--fg-1); font-weight: 500; }
  :global(.sh-ed__hl .sh-t-ref) { color: var(--accent); }
  :global(.sh-ed__hl .sh-t-num) { color: var(--fg-2); }
  :global(.sh-ed__hl .sh-t-op) { color: var(--fg-3); }
  /* Matt ruling 2026-09-05 #1 — same deviation as the formula bar: strings
     render green per the shipped `.sh-t-str` token, not "receding". */
  :global(.sh-ed__hl .sh-t-str) { color: var(--status-success); }
</style>
