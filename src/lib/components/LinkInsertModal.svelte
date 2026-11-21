<!--
  LinkInsertModal.svelte — #1898 PR8
  One-field dialog: paste/type a URL → dispatches `submit` with normalized URL.
  Parent calls insertAttachment({ attachmentType: 'link', src, renderMode: 'preview', attachmentId: null }).

  Design: ~/Shared/ExampleApp/docs/designs/1898-link-modal/ (lm-* CSS tokens)
  Scope: explicit-trigger only (toolbar chain-glyph). No paste interception.
-->

<script lang="ts">
  import { onMount, createEventDispatcher } from 'svelte';

  const dispatch = createEventDispatcher<{ submit: string; cancel: void }>();

  // ── Validation ────────────────────────────────────────────────────────────
  /**
   * Lenient URL check: empty → not valid; plausible URL shape (with or without
   * scheme) → valid; obvious garbage → invalid.
   *
   * Rules:
   *  - Empty string → { valid: false, error: false }
   *  - Has a dot/colon somewhere and no spaces → try URL constructor with
   *    https:// prepend if no scheme → { valid: true }
   *  - Otherwise → { valid: false, error: true }
   *
   * We do NOT gate on TLD lists. We do NOT fetch. We do NOT run DNS.
   */
  function classifyUrl(raw: string): { valid: boolean; error: boolean } {
    const trimmed = raw.trim();
    if (trimmed === '') return { valid: false, error: false };

    // Normalize: add https:// if no scheme present
    const candidate = /^[a-z][a-z0-9+\-.]*:\/\//i.test(trimmed)
      ? trimmed
      : `https://${trimmed}`;

    try {
      const u = new URL(candidate);
      // Must have a hostname with at least one dot (e.g. "pane.so") or localhost
      if (u.hostname === 'localhost' || u.hostname.includes('.')) {
        return { valid: true, error: false };
      }
      return { valid: false, error: true };
    } catch {
      return { valid: false, error: true };
    }
  }

  /**
   * Return the normalized URL string (with https:// prepended if needed).
   */
  function normalizeUrl(raw: string): string {
    const trimmed = raw.trim();
    if (/^[a-z][a-z0-9+\-.]*:\/\//i.test(trimmed)) return trimmed;
    return `https://${trimmed}`;
  }

  // ── State ─────────────────────────────────────────────────────────────────
  let inputValue = $state('');
  let focused = $state(false);

  const classification = $derived(classifyUrl(inputValue));
  const canAdd = $derived(classification.valid);
  const showError = $derived(classification.error);

  // ── DOM refs ──────────────────────────────────────────────────────────────
  let inputEl: HTMLInputElement;
  let modalEl: HTMLDivElement;

  onMount(() => {
    inputEl?.focus();
  });

  // ── Handlers ──────────────────────────────────────────────────────────────
  function handleSubmit() {
    if (!canAdd) return;
    dispatch('submit', normalizeUrl(inputValue));
  }

  function handleCancel() {
    dispatch('cancel');
  }

  function handleScrimClick(e: MouseEvent) {
    // Cancel only when the click lands on the scrim itself, not on the modal
    if (e.target === e.currentTarget) {
      handleCancel();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      handleCancel();
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="lm-scrim"
  role="dialog"
  aria-modal="true"
  aria-label="Insert link"
  onclick={handleScrimClick}
  onkeydown={handleKeydown}
>
  <div class="lm-modal" bind:this={modalEl} onclick={(e) => e.stopPropagation()}>
    <!-- Header -->
    <div class="lm-modal__head">
      <span class="lm-modal__icon" aria-hidden="true">
        <!-- Chain-link SVG (two arcs) -->
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/>
          <path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/>
        </svg>
      </span>
      <h3 class="lm-modal__title">Insert link</h3>
      <button class="lm-close" onclick={handleCancel} title="Cancel" aria-label="Cancel">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 6 6 18M6 6l12 12"/>
        </svg>
      </button>
    </div>

    <!-- URL input -->
    <div
      class="lm-input-wrap"
      class:lm-input-wrap--focus={focused}
      class:lm-input-wrap--error={showError}
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5"/>
        <path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19"/>
      </svg>
      <input
        bind:this={inputEl}
        bind:value={inputValue}
        type="text"
        placeholder="Paste a link…"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck={false}
        onfocus={() => (focused = true)}
        onblur={() => (focused = false)}
        aria-label="URL"
        data-testid="link-modal-input"
      />
    </div>

    <!-- Helper / error line -->
    {#if showError}
      <div class="lm-hint lm-hint--error" role="alert" data-testid="link-modal-error">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="9"/>
          <path d="M12 8v4M12 16h.01"/>
        </svg>
        That doesn't look like a URL.
      </div>
    {:else}
      <div class="lm-hint" data-testid="link-modal-hint">
        Adds a preview card. Switch to plain or embed later from the card.
      </div>
    {/if}

    <!-- Footer -->
    <div class="lm-actions">
      <button class="lm-btn lm-btn--ghost" onclick={handleCancel} data-testid="link-modal-cancel">
        Cancel
      </button>
      <button
        class="lm-btn lm-btn--primary"
        disabled={!canAdd}
        onclick={handleSubmit}
        data-testid="link-modal-add"
      >
        Add link <span class="lm-enter">↵</span>
      </button>
    </div>
  </div>
</div>

<style>
  /* ── Import design tokens from global CSS (lm-* classes live in link-modal.css
     which is imported in app.css/app-global, or we replicate the subset here) ── */

  .lm-scrim {
    position: fixed;
    inset: 0;
    background: rgba(5, 5, 7, 0.6);
    backdrop-filter: blur(3px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .lm-modal {
    width: 440px;
    box-sizing: border-box;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 20px;
    box-shadow: var(--shadow-pop);
  }

  .lm-modal__head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 15px;
  }

  .lm-modal__icon {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    flex-shrink: 0;
    background: var(--accent-soft);
    color: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .lm-modal__title {
    font-size: 15px;
    font-weight: 600;
    color: var(--fg-1);
    letter-spacing: -0.01em;
    margin: 0;
    flex: 1;
  }

  .lm-close {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    border: 0;
    background: transparent;
    color: var(--fg-3);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }

  .lm-close:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  .lm-input-wrap {
    display: flex;
    align-items: center;
    gap: 9px;
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 0 12px;
    background: var(--bg);
    transition: border-color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease);
  }

  .lm-input-wrap svg {
    color: var(--fg-4);
    flex-shrink: 0;
  }

  .lm-input-wrap--focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px rgba(0, 228, 228, 0.12);
  }

  .lm-input-wrap--focus svg {
    color: var(--accent);
  }

  .lm-input-wrap--error {
    border-color: var(--status-danger);
  }

  .lm-input-wrap--error svg {
    color: var(--status-danger);
  }

  .lm-input-wrap input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: none;
    color: var(--fg-1);
    font-family: var(--font-mono);
    font-size: 13.5px;
    letter-spacing: -0.01em;
    padding: 11px 0;
  }

  .lm-input-wrap input::placeholder {
    color: var(--fg-4);
  }

  .lm-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    margin: 8px 2px 0;
    min-height: 16px;
    color: var(--fg-4);
  }

  .lm-hint--error {
    color: var(--status-danger);
  }

  .lm-hint svg {
    flex-shrink: 0;
  }

  .lm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 18px;
  }

  .lm-btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-family: var(--font-sans);
    font-size: 13.5px;
    font-weight: 600;
    padding: 9px 15px;
    border-radius: 9px;
    border: 1px solid transparent;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }

  .lm-btn:active {
    transform: scale(0.97);
  }

  .lm-btn--ghost {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .lm-btn--ghost:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  .lm-btn--primary {
    background: var(--accent);
    color: var(--fg-on-accent);
  }

  .lm-btn--primary:hover {
    background: var(--accent-hover);
  }

  .lm-btn--primary:disabled {
    background: rgba(0, 228, 228, 0.28);
    color: rgba(10, 10, 15, 0.5);
    cursor: not-allowed;
  }

  .lm-enter {
    font-family: var(--font-mono);
    font-size: 10.5px;
    opacity: 0.7;
  }
</style>
