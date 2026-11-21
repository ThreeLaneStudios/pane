<script lang="ts">
  // SheetCellEditorDate.svelte — masked MM/DD/YYYY text editor for the
  // `date` column type (#2339 PR 10, ruling 4). Calendar-grid popover is
  // explicitly OUT OF SCOPE per ruling 4 — this is a plain text input with
  // a `/`-separated placeholder and a slash-auto-insert convenience as you
  // type digits. Conversion to ISO-8601 happens on commit, in
  // `sheetCellCommit.ts#parseDateMaskToIso` (this component doesn't know
  // about ISO at all — it just edits/displays the mask string the caller
  // hands it via `value`).

  interface Props {
    value: string;
    onCommit: (cause: 'enter' | 'tab' | 'shift-tab' | 'blur') => void;
    onCancel: () => void;
    /** #2874 item 12 Part D: which surface started THIS editing session
     *  — see `SheetCellEditorText.svelte`'s identical doc comment. When
     *  `'bar'`, skips this component's own `.focus()` call so the bar's
     *  own input wins the mount-order race instead. */
    origin?: 'cell' | 'bar';
  }

  let { value = $bindable(), onCommit, onCancel, origin = 'cell' }: Props = $props();

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

  // Lightweight typing helper: auto-insert '/' after the 2nd and 4th typed
  // digit so a bare-digit-typing user ends up with a well-formed mask
  // without having to type the separators themselves. Never fights a user
  // who types their own slashes.
  function handleInput(e: Event) {
    const el = e.target as HTMLInputElement;
    const digits = el.value.replace(/[^0-9]/g, '');
    if (el.value.length > value.length && /[0-9]$/.test(el.value) && !el.value.includes('/', el.value.length - 1)) {
      if (digits.length === 2 || digits.length === 4) {
        value = el.value + '/';
        return;
      }
    }
    value = el.value;
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
</script>

<div class="sh-ed sh-ed--mono" data-testid="sheet-cell-editor" data-variant="date">
  <input
    bind:this={inputEl}
    value={value}
    oninput={handleInput}
    type="text"
    inputmode="numeric"
    placeholder="MM/DD/YYYY"
    maxlength="10"
    class="sh-ed__input"
    onkeydown={handleKeydown}
    onblur={handleBlur}
    data-testid="sheet-cell-editor-input"
  />
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
  .sh-ed--mono { font-family: var(--font-mono); font-size: 12.5px; }

  .sh-ed__input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    color: inherit;
    font-family: inherit;
    font-size: inherit;
    padding: 0;
  }
  .sh-ed__input::placeholder { color: var(--fg-4); }
</style>
