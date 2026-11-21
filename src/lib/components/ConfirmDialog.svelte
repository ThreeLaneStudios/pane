<script lang="ts">
  /**
   * ConfirmDialog.svelte — Shared confirmation modal
   *
   * Extracted from Sidebar.svelte's inline project-delete dialog (#542).
   * Used for folder-delete, note-delete (NotesBrowser, AllTasksView), and project-delete.
   *
   * Props:
   *   title        — Dialog heading (e.g. "Delete Project")
   *   message      — Body text explaining consequences
   *   confirmLabel — Button label, default "Delete"
   *   confirmColor — Button background color, default "#EF4444" (red)
   *   error        — #2874 item 65 ruling 2: optional, visible-in-the-UI
   *                  failure message from a confirm attempt that errored.
   *                  Rendered below `message`, dialog stays open (caller's
   *                  onConfirm decides whether to close on success).
   *   onConfirm    — Called when user clicks confirm
   *   onCancel     — Called when user clicks cancel or overlay
   */

  interface Props {
    title: string;
    message: string;
    confirmLabel?: string;
    confirmColor?: string;
    error?: string | null;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    title,
    message,
    confirmLabel = 'Delete',
    confirmColor = '#EF4444',
    error = null,
    onConfirm,
    onCancel,
  }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onCancel();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="confirm-overlay"
  role="button"
  tabindex="-1"
  onclick={onCancel}
></div>

<div class="confirm-modal" role="dialog" aria-modal="true" aria-labelledby="confirm-title">
  <p id="confirm-title" class="confirm-title">{title}</p>
  <p class="confirm-message">{message}</p>
  {#if error}
    <p class="confirm-error">{error}</p>
  {/if}
  <div class="confirm-actions">
    <button class="confirm-btn cancel-btn" onclick={onCancel}>Cancel</button>
    <button
      class="confirm-btn action-btn"
      style:background={confirmColor}
      onclick={onConfirm}
    >
      {confirmLabel}
    </button>
  </div>
</div>

<style>
  .confirm-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    background: rgba(0, 0, 0, 0.5);
    z-index: 10000;
    cursor: default;
  }

  .confirm-modal {
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

  .confirm-title {
    color: #e0e0e0;
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 500;
  }

  .confirm-message {
    color: #666666;
    margin: 0 0 16px;
    font-size: 12px;
    line-height: 1.5;
  }

  .confirm-error {
    color: #EF4444;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 4px;
    padding: 8px 10px;
    margin: 0 0 16px;
    font-size: 12px;
    line-height: 1.4;
  }

  .confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .confirm-btn {
    border: none;
    padding: 6px 16px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
    transition: opacity 0.15s ease;
  }

  .confirm-btn:hover {
    opacity: 0.85;
  }

  .cancel-btn {
    background: rgba(255, 255, 255, 0.06);
    color: #888888;
  }

  .action-btn {
    color: white;
  }
</style>
