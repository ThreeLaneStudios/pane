<script lang="ts">
  /**
   * AttachmentModePicker.svelte — "Display as" popover for attachment blocks.
   *
   * Per-type mode list, active-mode check indicator, Open externally action.
   * Keyboard accessible (Esc closes, arrow keys navigate, Enter selects).
   * Click-outside closes.
   *
   * §401 PR3 — spec-covered-by: ADR-0049 §2.1A + ADR-0050 §6.
   *
   * Scope: mode selection + attribute persistence ONLY.
   * Expanded/media rendering is PR4–7.
   */

  import { onMount, onDestroy } from 'svelte';
  import { VALID_RENDER_MODES } from '$lib/extensions/Attachment';

  /** Human-readable mode labels per render mode value. */
  const MODE_LABELS: Record<string, string> = {
    'card':         'Card',
    'full-inline':  'Inline',
    'reader':       'Reader',
    'player':       'Player',
    'preview':      'Preview',
    'plain':        'Plain',
  };

  /**
   * Per-type, per-mode descriptions shown beneath each mode label in the picker.
   * Canonical strings sourced from attachment-screens.jsx MODES export (#1774 design).
   * Keyed by [attachmentType][renderMode]. Falls back to '' if a pair is undefined.
   *
   * NOTE: Design uses mode key 'full' with label 'Full size'; codebase uses
   * 'full-inline' with label 'Inline'. Keeping codebase key here; label change
   * deferred to a separate PR to avoid unplanned scope.
   */
  const MODE_DESCRIPTIONS: Record<string, Record<string, string>> = {
    link:  {
      'plain':   'Clickable URL, inline in your text.',
      'preview': 'Favicon, title and description.',
    },
    pdf: {
      'card':   'Icon, name, size and page count.',
      'reader': 'Open full-screen, scroll all pages.',
    },
    audio: {
      'card':   'Icon, name and size.',
      'player': 'Plays in the sidebar; audio continues while you browse.',
    },
    video: {
      'card':   'Icon, name and size.',
      'player': 'Embedded player, inline in the note.',
    },
    image: {
      'full-inline': 'Inline, drag a corner to resize.',
      'card':        'Thumbnail with name and size.',
    },
    html: {
      'card':    'Icon and file name.',
      'preview': 'Sandboxed full-screen render. No scripts or network.',
    },
  };

  interface Props {
    /** Attachment type (e.g. 'image', 'pdf', 'audio', 'video', 'html', 'link'). */
    attachmentType: string | null;
    /** Currently-active render mode. */
    currentMode: string | null;
    /** DOM element to position the popover against. */
    anchor?: HTMLElement | null;
    /** Called when the user selects a new mode. Receives the new mode string. */
    onModeChange?: (mode: string) => void;
    /** Called when the user clicks "Open externally". */
    onOpenExternally?: () => void;
    /** Called when the popover should close (Esc, click-outside, etc.). */
    onClose?: () => void;
  }

  let {
    attachmentType = null,
    currentMode = null,
    anchor = null,
    onModeChange,
    onOpenExternally,
    onClose,
  }: Props = $props();

  onMount(() => {
    // Focus first item after mount
    focusItem(0);
  });

  // ─── Mode list ───────────────────────────────────────────────────────────────
  /** Compute the visible mode list for the current attachment type. */
  let modes = $derived((() => {
    const type = attachmentType ?? 'file';
    return (VALID_RENDER_MODES[type] ?? ['card']) as readonly string[];
  })());

  // ─── Keyboard navigation ─────────────────────────────────────────────────
  let focusedIndex = $state(0);

  // Reset focused index when modes list changes
  $effect(() => {
    focusedIndex = 0;
    void modes; // reactive dep
  });

  /** Total navigable items: mode items + 1 action item (Open externally). */
  let totalItems = $derived(modes.length + 1);

  function focusItem(index: number) {
    focusedIndex = Math.max(0, Math.min(index, totalItems - 1));
    // Focus the DOM element on next tick
    setTimeout(() => {
      const el = popoverEl?.querySelectorAll<HTMLElement>('[data-picker-item]')[focusedIndex];
      el?.focus();
    }, 0);
  }

  function handleKeydown(e: KeyboardEvent) {
    switch (e.key) {
      case 'Escape':
        e.preventDefault();
        onClose?.();
        break;
      case 'ArrowDown':
        e.preventDefault();
        focusItem(focusedIndex + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        focusItem(focusedIndex - 1);
        break;
      case 'Enter': {
        e.preventDefault();
        // The focused element's click handler will fire
        const el = popoverEl?.querySelectorAll<HTMLElement>('[data-picker-item]')[focusedIndex];
        el?.click();
        break;
      }
    }
  }

  // ─── Click-outside ───────────────────────────────────────────────────────
  let popoverEl: HTMLElement | null = null;

  function handleClickOutside(e: MouseEvent) {
    if (popoverEl && !popoverEl.contains(e.target as Node)) {
      // Also check if the click was on the anchor button itself
      if (anchor && anchor.contains(e.target as Node)) return;
      onClose?.();
    }
  }

  onMount(() => {
    document.addEventListener('mousedown', handleClickOutside, true);
  });

  onDestroy(() => {
    document.removeEventListener('mousedown', handleClickOutside, true);
  });

  // ─── Position ────────────────────────────────────────────────────────────
  let posStyle = $state('');

  $effect(() => {
    if (!anchor || !popoverEl) return;
    const rect = anchor.getBoundingClientRect();
    // Position below the anchor, left-aligned
    posStyle = `top:${rect.bottom + 6}px;left:${rect.left}px;`;
  });

  // ─── Action handlers ─────────────────────────────────────────────────────
  function selectMode(mode: string) {
    if (mode !== currentMode) {
      onModeChange?.(mode);
    }
    onClose?.();
  }

  function handleOpenExternally() {
    onOpenExternally?.();
    onClose?.();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="amp-popover"
  style={posStyle}
  role="menu"
  aria-label="Display as"
  tabindex="-1"
  onkeydown={handleKeydown}
  bind:this={popoverEl}
>
  <!-- Section header -->
  <div class="amp-section-header" aria-hidden="true">Display as</div>

  <!-- Per-type mode list -->
  {#each modes as mode, i (mode)}
    {@const isActive = mode === currentMode}
    {@const descText = MODE_DESCRIPTIONS[attachmentType ?? '']?.[mode] ?? ''}
    <button
      class="amp-item amp-item--mode"
      class:is-active={isActive}
      role="menuitemradio"
      aria-checked={isActive}
      aria-label={MODE_LABELS[mode] ?? mode}
      tabindex={focusedIndex === i ? 0 : -1}
      data-picker-item
      data-mode={mode}
      onclick={() => selectMode(mode)}
      onfocus={() => { focusedIndex = i; }}
      type="button"
    >
      <!-- Cyan check icon for active mode -->
      <span class="amp-check" aria-hidden="true">
        {#if isActive}
          <!-- Lucide Check icon -->
          <svg
            class="amp-check-icon"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <polyline points="20 6 9 17 4 12"/>
          </svg>
        {/if}
      </span>
      <span class="amp-item-label">{MODE_LABELS[mode] ?? mode}</span>
      {#if descText}<span class="amp-item-desc">{descText}</span>{/if}
    </button>
  {/each}

  <!-- Separator between modes and actions -->
  <div class="amp-separator" role="separator" aria-hidden="true"></div>

  <!-- Action: Open externally -->
  <button
    class="amp-item amp-item--action"
    role="menuitem"
    aria-label="Open externally"
    tabindex={focusedIndex === modes.length ? 0 : -1}
    data-picker-item
    data-action="open-externally"
    onclick={handleOpenExternally}
    onfocus={() => { focusedIndex = modes.length; }}
    type="button"
  >
    <span class="amp-check" aria-hidden="true"></span>
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
      <polyline points="15 3 21 3 21 9"/>
      <line x1="10" y1="14" x2="21" y2="3"/>
    </svg>
    <span class="amp-item-label">Open externally</span>
  </button>
</div>

<style>
  .amp-popover {
    position: fixed;
    z-index: 200;
    min-width: 160px;
    padding: 5px;
    background: var(--surface-3, #1c1c20);
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    border-radius: 10px;
    box-shadow: var(--shadow-pop, 0 4px 24px rgba(0,0,0,0.45));
  }

  .amp-section-header {
    padding: 4px 10px 2px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--fg-3, rgba(255,255,255,0.35));
    user-select: none;
  }

  .amp-item {
    display: flex;
    align-items: flex-start;
    flex-wrap: wrap;
    gap: 2px 6px;
    width: 100%;
    padding: 5px 8px;
    background: transparent;
    border: 0;
    border-radius: 6px;
    color: var(--fg-2, rgba(255,255,255,0.65));
    cursor: pointer;
    font-family: var(--font-sans, inherit);
    font-size: 13px;
    font-weight: 450;
    text-align: left;
    transition:
      background var(--dur-fast, 120ms) ease,
      color var(--dur-fast, 120ms) ease;
    outline: none;
  }
  .amp-item:hover,
  .amp-item:focus {
    background: var(--overlay-press, rgba(255,255,255,0.08));
    color: var(--fg-1, #e0e0e0);
  }
  .amp-item.is-active {
    color: var(--fg-1, #e0e0e0);
  }
  .amp-item--action {
    color: var(--fg-2, rgba(255,255,255,0.6));
  }

  /* Cyan check icon slot — fixed width so labels align */
  .amp-check {
    width: 16px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* Cyan color for the active-check icon (design token; fallback to teal) */
  .amp-check-icon {
    color: var(--accent-cyan, #06b6d4);
    stroke: var(--accent-cyan, #06b6d4);
  }

  .amp-item-label {
    flex: 1;
  }

  .amp-item-desc {
    display: block;
    width: 100%;
    font-size: 11px;
    font-weight: 400;
    color: var(--fg-3, rgba(255,255,255,0.35));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-top: 1px;
    line-height: 1.3;
  }

  .amp-separator {
    height: 1px;
    background: var(--border, rgba(255,255,255,0.08));
    margin: 4px 0;
  }
</style>
