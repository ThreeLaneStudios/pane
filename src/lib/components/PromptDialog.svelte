<script lang="ts">
  /**
   * PromptDialog.svelte — Shared text-input prompt modal
   *
   * Replaces native `window.prompt()` calls, which don't render in Tauri's
   * WebView. Modeled on ConfirmDialog.svelte's state-variable + callback
   * pattern (NOT a Promise-wrapper) for consistency with the rest of the
   * codebase (#2874 item 6).
   *
   * Props:
   *   title        — Dialog heading (e.g. "New Folder")
   *   message      — Optional body text (some prompts are self-explanatory
   *                  from the title alone, e.g. "Insert Link")
   *   placeholder  — Input placeholder text
   *   initialValue — Pre-fill the input (selected on mount if non-empty)
   *   confirmLabel — Button label, default "OK"
   *   cancelLabel  — Button label, default "Cancel"
   *   confirmColor — Button background color, default "#00E4E4" (teal) —
   *                  none of these actions are destructive, so this should
   *                  NOT default to ConfirmDialog's red.
   *   validate     — Optional custom validity check; if omitted, the confirm
   *                  button is disabled when the trimmed input is empty.
   *   onConfirm    — Called with the trimmed input value
   *   onCancel     — Called when user clicks cancel, presses Escape, or
   *                  clicks the overlay
   */

  import { onMount } from 'svelte';

  interface Props {
    title: string;
    message?: string;
    placeholder?: string;
    initialValue?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    confirmColor?: string;
    validate?: (value: string) => boolean;
    onConfirm: (value: string) => void;
    onCancel: () => void;
  }

  let {
    title,
    message,
    placeholder = '',
    initialValue = '',
    confirmLabel = 'OK',
    cancelLabel = 'Cancel',
    confirmColor = '#00E4E4',
    validate,
    onConfirm,
    onCancel,
  }: Props = $props();

  let inputValue = $state(initialValue);
  let inputEl: HTMLInputElement | null = $state(null);

  const trimmed = $derived(inputValue.trim());
  const isValid = $derived(validate ? validate(inputValue) : trimmed.length > 0);

  function handleConfirm() {
    if (!isValid) return;
    onConfirm(trimmed);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onCancel();
  }

  function handleInputKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleConfirm();
    }
  }

  onMount(() => {
    inputEl?.focus();
    if (initialValue.trim().length > 0) {
      inputEl?.select();
    }
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="prompt-overlay"
  role="button"
  tabindex="-1"
  onclick={onCancel}
></div>

<div class="prompt-modal" role="dialog" aria-modal="true" aria-labelledby="prompt-title">
  <p id="prompt-title" class="prompt-title">{title}</p>
  {#if message}<p class="prompt-message">{message}</p>{/if}
  <input
    bind:this={inputEl}
    bind:value={inputValue}
    type="text"
    {placeholder}
    class="prompt-input"
    onkeydown={handleInputKeydown}
  />
  <div class="prompt-actions">
    <button class="prompt-btn cancel-btn" onclick={onCancel}>{cancelLabel}</button>
    <button
      class="prompt-btn action-btn"
      style:background={confirmColor}
      disabled={!isValid}
      onclick={handleConfirm}
    >
      {confirmLabel}
    </button>
  </div>
</div>

<style>
  .prompt-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    background: rgba(0, 0, 0, 0.5);
    z-index: 10000;
    cursor: default;
  }

  .prompt-modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: #1E1E20;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 8px;
    padding: 20px;
    z-index: 10001;
    min-width: 300px;
    max-width: 420px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }

  .prompt-title {
    color: #e0e0e0;
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 500;
  }

  .prompt-message {
    color: #666666;
    margin: 0 0 16px;
    font-size: 12px;
    line-height: 1.5;
  }

  .prompt-input {
    width: 100%;
    box-sizing: border-box;
    background: #2C2C2E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    padding: 8px 10px;
    color: #e0e0e0;
    font-size: 13px;
    font-family: inherit;
    margin-bottom: 16px;
    outline: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
  }

  .prompt-input::placeholder {
    color: #666666;
  }

  .prompt-input:focus {
    border-color: #00E4E4;
    box-shadow: 0 0 0 3px rgba(0, 228, 228, 0.12);
  }

  .prompt-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .prompt-btn {
    border: none;
    padding: 6px 16px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
    transition: opacity 0.15s ease;
  }

  .prompt-btn:hover {
    opacity: 0.85;
  }

  .cancel-btn {
    background: rgba(255, 255, 255, 0.06);
    color: #888888;
  }

  .action-btn {
    color: white;
  }

  .action-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .action-btn:disabled:hover {
    opacity: 0.4;
  }
</style>
