<script lang="ts">
  // FormulaBar.svelte — #2339 PR 14 (Formula Bar & Formula Editing),
  // per docs/design/sheets/sheets-shared.jsx's `FormulaBar` component and
  // sheets.css's `.sh-fbar*` classes (ported verbatim below).
  //
  // Mounted in Editor.svelte between SheetToolbar and SheetGrid. Purely a
  // DISPLAY/edit-buffer surface for the currently-anchored cell — it owns
  // NO backend calls of its own; `SheetGrid.svelte` (via bind:this exports)
  // is the single source of truth for the anchor cell's identity/value and
  // the single place a commit actually reaches `update_sheet_cell`. This
  // component and the in-cell formula editor are "one formula, two places
  // to edit it" (design Cap prose) — both bind the SAME buffer string, via
  // `value = $bindable()`, so keystrokes in either place are visible in
  // both instantly.
  //
  // Token coloring: `fx` here is a static label, never a button (Matt
  // ruling 2026-09-05 #5 — superseded by #2874 item 16 Part C, which
  // wires a SEPARATE fx button on `SheetToolbar` instead, via
  // `SheetGrid.svelte`'s `openFormulaOnAnchor()`; this bar's own `fx`
  // label remains static, unrelated to that toolbar button). String
  // tokens (`.sh-t-str`) render in `var(--status-success)` GREEN — the
  // design prose says "numbers and strings recede," but the locked
  // `sheets.css` itself gives `.sh-t-str` the success-green token, and
  // Matt ruled 2026-09-05 that the shipped CSS wins over the prose. This
  // is a deliberate, cited deviation, not an oversight.
  import { tokenizeFormula } from '$lib/utils/sheetFormula';

  interface Props {
    /** A1 address of the anchor cell (e.g. "F3"), or empty when nothing is
     *  selected (an empty sheet with no columns/rows). */
    cref: string;
    /** Live edit buffer — bound two-way so SheetGrid's in-cell formula
     *  editor and this bar can share one buffer/caret when both are open
     *  for the same cell. */
    value: string;
    /** True while the anchor cell is in formula/value-editing mode (drives
     *  `.sh-fbar.is-editing` and the live caret). */
    editing?: boolean;
    /** Inline rejection message (formula parse failure or an out-of-bounds
     *  A1 reference per Matt ruling 2026-09-05 #3) — same UX language as
     *  PR 10's in-cell `editorError`, adapted to the bar's own layout. */
    error?: string | null;
    /** #2339 PR 14 fix (bar/cell caret parity): the shared editing
     *  session's live caret position, mirroring `SheetGrid.svelte`'s
     *  `EditingCell.selectionStart`/`selectionEnd`. When focus moves from
     *  the in-cell editor to this bar mid-edit (or vice versa), these let
     *  the newly-focused surface restore the caret to where the user left
     *  it instead of always jumping to the end — `null` (nothing recorded
     *  yet, e.g. a session that opened directly in the bar) falls back to
     *  end-of-text, matching the pre-existing behavior. */
    selectionStart?: number | null;
    selectionEnd?: number | null;
    onInput?: (next: string) => void;
    onCommit?: (cause: 'enter' | 'blur') => void;
    onCancel?: () => void;
    /** Fired on every caret/selection change while editing — the other
     *  half of the `selectionStart`/`selectionEnd` props above. `SheetGrid`
     *  stores whatever this reports on the shared `editing` session so the
     *  in-cell editor can pick up the same caret if focus moves there. */
    onSelectionChange?: (start: number, end: number) => void;
    /** #2874 item 12: fired when the user clicks the IDLE bar (the
     *  tokenized display span or the "Empty cell" placeholder) — i.e.
     *  BEFORE `editing` is true and before `inputEl` exists. The parent
     *  is expected to call `sheetGridRef.barBeginEdit()` in response,
     *  which flips `editing` to true and causes this component to
     *  re-render into its editing branch on the next reactive pass. */
    onBarClick?: () => void;
  }

  let {
    cref,
    value = $bindable(),
    editing = false,
    error = null,
    selectionStart = null,
    selectionEnd = null,
    onInput,
    onCommit,
    onCancel,
    onSelectionChange,
    onBarClick,
  }: Props = $props();

  // Matt ruling 2026-09-05 #7: the chevron toggles the bar between a
  // single-line and a three-line expanded height for long formulas — a
  // real interaction, not a static decoration. Collapses automatically
  // when editing ends so a later, shorter formula doesn't inherit a
  // stale expanded height.
  let expanded = $state(false);

  function toggleExpanded() {
    expanded = !expanded;
  }

  $effect(() => {
    if (!editing) expanded = false;
  });

  let inputEl: HTMLInputElement | null = $state(null);
  let barRootEl: HTMLDivElement | null = $state(null);

  // #2339 PR 14 fix (bar/cell caret parity): focusing the bar restores
  // whatever caret/selection the shared editing session last recorded
  // (`selectionStart`/`selectionEnd`, carried over from the in-cell
  // editor if that's where the edit started) rather than always jumping
  // to end-of-text — `null` on either prop (no recorded selection yet)
  // falls back to the previous end-of-text behavior.
  //
  // #2874 P0-1 fix: guarded to a RISING EDGE of `editing` (false -> true)
  // only, so a value-driven re-render while already editing (e.g. every
  // keystroke landing in the in-cell editor, which also reassigns this
  // same `editing` prop via SheetGrid's onSelectionInfo -> Editor $state
  // -> FormulaBar re-render chain) does not re-steal focus from whichever
  // surface (bar or in-cell editor) currently owns it. On the rising edge
  // itself, ALSO check whether focus already landed somewhere real (not
  // this bar's own input, not <body>) before calling .focus() -- when a
  // cell is opened via double-click/type-to-edit, the in-cell editor's
  // own identical focus-effect (SheetCellEditorText/.../Formula, all
  // still unguarded per Matt's ruling) races this one in the SAME
  // reactive flush and typically wins by DOM order; without this check,
  // the bar would still steal focus back on that very first transition.
  let prevEditing = false;
  $effect(() => {
    const activeEl = document.activeElement as Element | null;
    // #2874 item 12 fix: a click that landed anywhere INSIDE this bar's
    // own container (e.g. the idle display/placeholder span that
    // `onBarClick` fires from) still has that same element as
    // `document.activeElement` on this first reactive pass — BEFORE this
    // effect's own `inputEl.focus()` call below runs — so without this
    // check it would incorrectly read as "focus already elsewhere" and
    // skip focusing the input. Treat that case as NOT elsewhere.
    const focusInsideBar = !!activeEl && !!barRootEl && barRootEl.contains(activeEl);
    const focusAlreadyElsewhere = !!activeEl && activeEl !== inputEl && activeEl.tagName !== 'BODY' && !focusInsideBar;
    if (editing && inputEl && !prevEditing && !focusAlreadyElsewhere) {
      inputEl.focus();
      const len = inputEl.value.length;
      const start = selectionStart ?? len;
      const end = selectionEnd ?? len;
      inputEl.setSelectionRange(Math.min(start, len), Math.min(end, len));
    }
    prevEditing = editing;
  });

  function handleInput(e: Event) {
    const next = (e.target as HTMLInputElement).value;
    value = next;
    onInput?.(next);
    reportSelection();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      onCommit?.('enter');
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onCancel?.();
    }
  }

  function handleBlur() {
    if (editing) onCommit?.('blur');
  }

  /** Reports the input's current caret/selection up to the parent so it
   *  can carry it over if focus moves to the in-cell editor next — the
   *  other half of the `selectionStart`/`selectionEnd` props' restore
   *  effect above. Fired on every event that can move the caret without
   *  necessarily changing `value` (click, arrow keys, select-all, etc.),
   *  not just `oninput`. */
  function reportSelection() {
    if (!inputEl) return;
    onSelectionChange?.(inputEl.selectionStart ?? 0, inputEl.selectionEnd ?? 0);
  }

  let tokens = $derived(tokenizeFormula(value ?? ''));
</script>

<div bind:this={barRootEl} class="sh-fbar" class:is-editing={editing} class:is-expanded={expanded} data-testid="sheet-formula-bar">
  <div class="sh-fbar__ref" data-testid="sheet-formula-bar-ref">{cref}</div>
  <div class="sh-fbar__fx">fx</div>
  <div class="sh-fbar__in">
    {#if editing}
      <!-- #2339 PR 14 fix (bar/cell tokenizing parity): same transparent-
           input-over-colored-overlay technique as
           `SheetCellEditorFormula.svelte`'s `.sh-ed__hl`/`.sh-ed__input`
           pair — "the same tokenizing and the same caret" for bar and
           cell (design prose, stated three times). The overlay renders
           the SAME `tokenizeFormula` tokens/classes the bar's own
           non-editing display below already uses; only the real
           (caret-visible-but-text-transparent) input receives
           keystrokes/focus/selection. -->
      <div class="sh-fbar__edit-wrap">
        <div class="sh-fbar__hl" aria-hidden="true" data-testid="sheet-formula-bar-highlight">
          {#each tokens as tok, i (i)}<span class={tok.cls}>{tok.text}</span>{/each}
        </div>
        <input
          bind:this={inputEl}
          class="sh-fbar__input"
          type="text"
          value={value ?? ''}
          oninput={handleInput}
          onkeydown={handleKeydown}
          onblur={handleBlur}
          onclick={reportSelection}
          onkeyup={reportSelection}
          onselect={reportSelection}
          data-testid="sheet-formula-bar-input"
        />
      </div>
    {:else if value}
      <span
        data-testid="sheet-formula-bar-display"
        onclick={onBarClick}
        role="button"
        tabindex="0"
        onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onBarClick?.(); } }}
      >
        {#each tokens as tok, i (i)}<span class={tok.cls}>{tok.text}</span>{/each}
      </span>
    {:else}
      <span
        class="sh-fbar__empty"
        onclick={onBarClick}
        role="button"
        tabindex="0"
        onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onBarClick?.(); } }}
      >Empty cell</span>
    {/if}
    {#if error}
      <div class="sh-fbar__err" data-testid="sheet-formula-bar-error">{error}</div>
    {/if}
  </div>
  <button
    class="sh-fbar__x"
    class:is-expanded={expanded}
    onclick={toggleExpanded}
    aria-label={expanded ? 'Collapse formula bar' : 'Expand formula bar'}
    aria-pressed={expanded}
    data-testid="sheet-formula-bar-chevron"
  >
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg>
  </button>
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css's `.sh-fbar*` /
     `.sh-t-*` / `.sh-caret` sections. */
  .sh-fbar { display: flex; align-items: stretch; gap: 0; border-bottom: 1px solid var(--border-faint); flex-shrink: 0; background: var(--bg); position: relative; }
  .sh-fbar__ref { width: 72px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-size: 12.5px; font-weight: 500; color: var(--fg-1); border-right: 1px solid var(--border-faint); font-variant-numeric: tabular-nums; }
  .sh-fbar__fx { width: 34px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-size: 12px; font-style: italic; color: var(--fg-4); border-right: 1px solid var(--border-faint); }
  .sh-fbar__in { flex: 1; min-width: 0; display: flex; align-items: center; padding: 9px 12px; font-family: var(--font-mono); font-size: 12.5px; color: var(--fg-2); line-height: 1.5; overflow: hidden; white-space: nowrap; position: relative; }
  .sh-fbar.is-editing { background: var(--surface-1); }
  .sh-fbar.is-editing .sh-fbar__in { box-shadow: inset 0 0 0 1px var(--accent); color: var(--fg-1); }
  .sh-fbar__empty { color: var(--fg-4); font-family: var(--font-sans); font-size: 12.5px; }
  .sh-fbar__x { width: 34px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: var(--fg-4); border-left: 1px solid var(--border-faint); background: transparent; border-top: 0; border-bottom: 0; border-right: 0; cursor: pointer; }
  .sh-fbar__x:hover { color: var(--fg-2); background: var(--surface-2); }
  .sh-fbar__x.is-expanded { color: var(--accent); }
  /* Implementation-only: the chevron rotates when expanded, signalling
     "collapse" instead of "expand" — the design's static mockup only
     shows the single collapsed-state glyph. */
  .sh-fbar__x svg { transition: transform var(--dur-fast, 0.12s) var(--ease, ease); }
  .sh-fbar__x.is-expanded svg { transform: rotate(180deg); }

  :global(.sh-t-fn) { color: var(--fg-1); font-weight: 500; }
  :global(.sh-t-ref) { color: var(--accent); }
  :global(.sh-t-num) { color: var(--fg-2); }
  :global(.sh-t-op) { color: var(--fg-3); }
  /* Matt ruling 2026-09-05 #1: CSS-as-authored wins over the design
     prose's "numbers and strings recede" — strings render in the
     success-green token, matching the shipped sheets.css exactly. */
  :global(.sh-t-str) { color: var(--status-success); }

  /* #2339 PR 14 fix (bar/cell tokenizing parity): the same highlight-
     behind-transparent-input technique as
     `SheetCellEditorFormula.svelte`'s `.sh-ed__hl`/`.sh-ed__input` pair,
     ported to the bar's own single-line layout — the overlay and the
     input share identical box metrics (font, padding via the shared
     parent) and are stacked via `position: relative`/`absolute`, so the
     colored spans sit exactly under the typed characters. Only the REAL
     input receives keystrokes/caret/selection; the overlay is
     `pointer-events: none` and `aria-hidden`, purely visual. */
  .sh-fbar__edit-wrap { position: relative; flex: 1; min-width: 0; height: 100%; display: flex; align-items: center; }
  .sh-fbar__hl {
    position: absolute;
    left: 0;
    right: 0;
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
  /* Implementation-only input reset — matches SheetCellEditorText.svelte's
     own `.sh-ed__input` treatment so the live-editing buffer looks
     identical whether it's rendered here or in the grid. */
  .sh-fbar__input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    font-family: inherit;
    font-size: inherit;
    padding: 0;
    position: relative;
    z-index: 1;
    /* The real input's own glyphs are invisible — only its caret shows —
       so the colored overlay underneath is what the user actually reads,
       matching `SheetCellEditorFormula.svelte`'s `.sh-ed__input` exactly. */
    color: transparent;
    caret-color: var(--accent);
  }

  /* Matt ruling 2026-09-05 #7: chevron-triggered three-line expansion for
     long formulas. Not in the static design mockup (which only shows the
     collapsed single-line state) — height/wrap values are this
     implementation's own choice, following the same line-height already
     set on `.sh-fbar__in` (1.5 × 12.5px font ≈ 19px per line + the
     existing 9px top/bottom padding). */
  .sh-fbar.is-expanded { align-items: flex-start; }
  .sh-fbar.is-expanded .sh-fbar__in { white-space: normal; word-break: break-word; max-height: calc(1.5em * 3 + 18px); overflow-y: auto; align-items: flex-start; }
  .sh-fbar.is-expanded .sh-fbar__ref,
  .sh-fbar.is-expanded .sh-fbar__fx,
  .sh-fbar.is-expanded .sh-fbar__x { align-items: flex-start; padding-top: 9px; }

  /* PR 10's inline-rejection language (`.sh-ed__err`), adapted to the
     bar's own layout — Matt ruling 2026-09-05 #3/#8's "same UX surface as
     a parse rejection" requirement. Positioned under the input rather
     than under a grid cell. */
  .sh-fbar__err {
    position: absolute;
    left: 12px;
    top: 100%;
    margin-top: 4px;
    font-size: 11px;
    color: var(--status-danger);
    background: var(--surface-3);
    border: 1px solid rgba(239, 68, 68, 0.35);
    border-radius: var(--radius-sm);
    padding: 3px 7px;
    z-index: 22;
    white-space: nowrap;
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
