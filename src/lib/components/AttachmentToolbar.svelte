<script lang="ts">
  /**
   * AttachmentToolbar.svelte — Floating toolbar docked to the top of an attachment block.
   *
   * Shown on: hover (via parent .at-block:hover CSS), keyboard focus, or TipTap
   * node selection (selected={true} prop → adds .is-selected on the parent block,
   * which the CSS uses to reveal the toolbar).
   *
   * PR3 scope:
   *  - Mode-label button is now INTERACTIVE: click opens AttachmentModePicker.
   *  - Open externally button is wired (previously disabled placeholder).
   *  - Bubbles modeChange, openExternally events up to the NodeView.
   *
   * §401 PR3 — LOCKED-DESIGN-SPEC.md toolbar spec.
   */

  import AttachmentModePicker from './AttachmentModePicker.svelte';

  /** Human-readable mode labels per render mode value. */
  const MODE_LABELS: Record<string, string> = {
    'card':         'Card',
    'full-inline':  'Inline',
    'reader':       'Reader',
    'player':       'Player',
    'preview':      'Preview',
    'plain':        'Plain',
  };

  interface Props {
    /** Current render mode attribute from the TipTap node. */
    renderMode?: string | null;
    /** Attachment type from TipTap node attrs. */
    attachmentType?: string | null;
    /** Whether the TipTap node is currently selected. */
    selected?: boolean;
    /** Called when the user changes the render mode via the picker. */
    onModeChange?: (mode: string) => void;
    /** Called when the user requests open externally. */
    onOpenExternally?: () => void;
  }

  let {
    renderMode = 'card',
    attachmentType = null,
    selected = false,
    onModeChange,
    onOpenExternally,
  }: Props = $props();

  let modeLabel = $derived(
    renderMode ? (MODE_LABELS[renderMode] ?? renderMode) : 'Card'
  );

  // ─── Picker state ─────────────────────────────────────────────────────────
  let pickerOpen = $state(false);
  let modeBtnEl: HTMLElement | null = $state(null);

  function togglePicker() {
    pickerOpen = !pickerOpen;
  }

  function closePicker() {
    pickerOpen = false;
  }

  function handleModeChange(mode: string) {
    onModeChange?.(mode);
    pickerOpen = false;
  }
</script>

<!--
  The toolbar is positioned absolute, anchored to the top of the block.
  Visibility is controlled by CSS (.at-block:hover, .at-block.is-selected).
  We also expose an `is-open` class on hover for keyboard-accessible reveal.
  The `selected` prop is consumed by the parent .at-block wrapper in AttachmentNodeView.
-->
<div
  class="at-toolbar"
  class:is-open={selected}
  role="toolbar"
  aria-label="Attachment options"
>
  <!--
    Mode label button: opens the mode picker popover on click.
    PR3: now interactive (replaces the static span from PR2).
  -->
  <button
    class="at-tbtn at-tbtn--mode"
    class:is-picker-open={pickerOpen}
    aria-label={`Display mode: ${modeLabel}`}
    aria-expanded={pickerOpen}
    aria-haspopup="menu"
    type="button"
    bind:this={modeBtnEl}
    onclick={togglePicker}
  >
    <!-- Layers icon -->
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="m12 2 9 5-9 5-9-5z"/>
      <path d="m3 12 9 5 9-5"/>
      <path d="m3 17 9 5 9-5"/>
    </svg>
    {modeLabel}
    <!-- Chevron-down: indicates the picker is available. -->
    <span class="at-chev" aria-hidden="true">
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
        <path d="m6 9 6 6 6-6"/>
      </svg>
    </span>
  </button>

  <!-- Mode picker popover (mounted when open) -->
  {#if pickerOpen}
    <AttachmentModePicker
      {attachmentType}
      currentMode={renderMode}
      anchor={modeBtnEl}
      onModeChange={handleModeChange}
      onOpenExternally={() => { onOpenExternally?.(); closePicker(); }}
      onClose={closePicker}
    />
  {/if}

  <!-- Divider -->
  <span class="at-tdiv" aria-hidden="true"></span>

  <!-- Open externally button (wired in PR3) -->
  <button
    class="at-tbtn"
    title="Open externally"
    aria-label="Open externally"
    type="button"
    onclick={() => onOpenExternally?.()}
  >
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
      <polyline points="15 3 21 3 21 9"/>
      <line x1="10" y1="14" x2="21" y2="3"/>
    </svg>
  </button>
</div>

<style>
  /*
   * Floating toolbar — docked top-right of the block, straddling the top edge.
   * Hidden until .at-block:hover / .at-block.is-selected (or .is-open).
   * This component's style is scoped; the reveal logic lives on the parent .at-block.
   */
  .at-toolbar {
    position: absolute;
    top: -16px;
    right: 10px;
    z-index: 6;
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 4px;
    background: var(--surface-3, #1c1c20);
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    border-radius: 9px;
    box-shadow: var(--shadow-pop, 0 4px 16px rgba(0,0,0,0.4));
    opacity: 0;
    transform: translateY(3px);
    transition:
      opacity var(--dur-fast, 120ms) var(--ease, ease),
      transform var(--dur-fast, 120ms) var(--ease, ease);
    pointer-events: none;
  }

  /*
   * Reveal via .is-open (selection prop) OR parent hover (handled by
   * AttachmentNodeView wrapping .at-block and applying :hover).
   */
  .at-toolbar.is-open {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }

  /* Toolbar buttons */
  .at-tbtn {
    height: 28px;
    min-width: 28px;
    padding: 0 7px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: transparent;
    border: 0;
    border-radius: 6px;
    color: var(--fg-2, rgba(255,255,255,0.6));
    cursor: pointer;
    font-family: var(--font-sans, inherit);
    font-size: 12.5px;
    font-weight: 500;
    transition:
      background var(--dur-fast, 120ms) var(--ease, ease),
      color var(--dur-fast, 120ms) var(--ease, ease);
    white-space: nowrap;
  }
  .at-tbtn:not([disabled]):hover {
    background: var(--overlay-press, rgba(255,255,255,0.08));
    color: var(--fg-1, #e0e0e0);
  }
  .at-tbtn[disabled] {
    cursor: default;
    opacity: 0.5;
  }
  .at-tbtn--mode {
    color: var(--fg-1, #e0e0e0);
  }
  .at-tbtn--mode.is-picker-open {
    background: var(--overlay-press, rgba(255,255,255,0.08));
  }
  .at-tbtn svg {
    flex-shrink: 0;
  }
  .at-chev {
    color: var(--fg-3, rgba(255,255,255,0.4));
    margin-left: -2px;
    display: flex;
    align-items: center;
  }

  /* Divider between mode label and action buttons */
  .at-tdiv {
    width: 1px;
    height: 18px;
    background: var(--border, rgba(255,255,255,0.1));
    margin: 0 2px;
    flex-shrink: 0;
  }
</style>
