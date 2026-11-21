<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import * as api from '$lib/api';
  import type { TrashItem } from '$lib/types';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import {
    ENTITY_NOTE,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();
  let trashItems: TrashItem[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    await loadTrash();
  });

  // #2972 stage 4 (§34 inventory): Trash was never in §34.12's nine
  // screens and had NO `pane-data-changed` listener of any kind — a note
  // moved to trash by an agent/background tool call while this screen was
  // already open never appeared until the user closed and reopened it.
  // `api.listTrash()` has no single-item read counterpart (there is no
  // `getTrashItem(id)`), and the trash list is bounded/small (30-day
  // retention window) — the same "tiny, no per-item read, deliberate"
  // rationale Sidebar/ProjectsView already use for their `category`
  // branch applies here too, so a full `loadTrash()` re-read on a
  // `note`/`deleted` event is the correct patch shape, not a violation of
  // §34.5's "never full-collection refetch" discipline (that rule is about
  // the PROJECTS/NOTES store, not an already-small, bounded trash list).
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_type, reason } = payload;
    if (entity_type !== ENTITY_NOTE || reason !== REASON_DELETED) return;
    await loadTrash();
  }

  const unlistenDataChanged = subscribeDataChanged((payload) => {
    handleDataChangedEvent(payload);
  });

  onDestroy(() => {
    unlistenDataChanged.then((fn) => fn());
  });

  async function loadTrash() {
    try {
      loading = true;
      trashItems = await api.listTrash();
    } catch (error) {
      console.error('Failed to load trash:', error);
    } finally {
      loading = false;
    }
  }

  async function handleRestore(item: TrashItem) {
    try {
      const { contentWarning } = await api.restoreFromTrash(item.id);
      await loadTrash(); // Refresh the list
      // #2874 item 1, Matt's ruling 5: the file may have been missing when
      // this note was restored (rebuilt from a saved version, or lost
      // entirely with no snapshot to recover from) — that must reach the
      // user as an unambiguous message, never a silent, normal-looking
      // restore.
      if (contentWarning) {
        alert(contentWarning);
      }
    } catch (error) {
      console.error('Failed to restore item:', error);
      alert('Failed to restore item');
    }
  }

  // #2874 item G: Trash's Delete Permanently (and Empty Trash, which shared
  // this same hand-rolled modal/state machine) now go through the shared
  // ConfirmDialog.svelte component — same one Sidebar/NotesBrowser/
  // AllTasksView/KanbanBoard/NewProviderModal/Onboarding/ProviderConnectionCard
  // already use — instead of a forked, visually-broken (`rgba(255,255,255,0.05)`
  // background, effectively invisible) local copy. confirmTitle/confirmError
  // are new; confirmAction/confirmMessage/showConfirm/cancelConfirm keep their
  // existing shape so handleDeletePermanently/handleEmptyTrash barely change.
  let confirmAction: (() => Promise<void>) | null = $state(null);
  let confirmTitle = $state('');
  let confirmMessage = $state('');
  let confirmError: string | null = $state(null);

  function showConfirm(title: string, message: string, action: () => Promise<void>) {
    confirmTitle = title;
    confirmMessage = message;
    confirmError = null;
    confirmAction = action;
  }

  function cancelConfirm() {
    confirmAction = null;
    confirmTitle = '';
    confirmMessage = '';
    confirmError = null;
  }

  // #2874 item G: errors from the confirmed action now surface in the
  // dialog itself (ConfirmDialog's `error` prop, #3040) instead of only a
  // console.error that closes the dialog as if nothing happened — same
  // build-standard "every catch must surface the error" rule Sidebar's
  // confirmDeleteFolder error-prop usage already follows. The dialog stays
  // open on failure so the user can retry or cancel explicitly.
  async function executeConfirm() {
    if (!confirmAction) return;
    try {
      await confirmAction();
      confirmAction = null;
      confirmTitle = '';
      confirmMessage = '';
      confirmError = null;
    } catch (error) {
      console.error('[TRASH] Confirm action failed:', error);
      confirmError = error instanceof Error ? error.message : 'Action failed. Please try again.';
    }
  }

  function handleDeletePermanently(item: TrashItem) {
    showConfirm(
      'Delete Permanently?',
      `Permanently delete "${item.title || item.slug}"? This cannot be undone.`,
      async () => {
        await api.deletePermanently(item.id);
        await loadTrash();
      }
    );
  }

  function handleEmptyTrash() {
    showConfirm(
      'Empty Trash?',
      'Empty trash? All items will be permanently deleted. This cannot be undone.',
      async () => {
        const { failed } = await api.emptyTrash();
        await loadTrash();
        // #2874 item 2 (#3037 follow-up, Matt's ruling): an item whose file
        // couldn't be moved to the OS trash stays in Pane's own Trash rather
        // than being silently erased — the user must be told how many, same
        // non-blocking `alert()` pattern `handleRestore`'s content_warning
        // already uses on this screen.
        if (failed > 0) {
          alert(`${failed} item${failed === 1 ? '' : 's'} could not be permanently deleted and will stay in Trash.`);
        }
      }
    );
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      onClose();
    }
  }

  function formatDate(dateString: string): string {
    try {
      return new Date(dateString).toLocaleDateString('en-US', {
        year: 'numeric',
        month: 'short',
        day: 'numeric'
      });
    } catch {
      return dateString;
    }
  }

  function getDaysInTrash(deletedAt: string): number {
    try {
      const deleted = new Date(deletedAt);
      const now = new Date();
      const diffMs = now.getTime() - deleted.getTime();
      return Math.floor(diffMs / (1000 * 60 * 60 * 24));
    } catch {
      return 0;
    }
  }

  function getDaysRemaining(deletedAt: string): number {
    return Math.max(0, 30 - getDaysInTrash(deletedAt));
  }

  function getOriginalFolder(item: TrashItem): string {
    // Extract folder from the original path or use notebook as fallback
    if (item.original_path && item.original_path.includes('/')) {
      const pathParts = item.original_path.split('/');
      return pathParts[pathParts.length - 2] || item.notebook;
    }
    return item.notebook;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if confirmAction}
  <ConfirmDialog
    title={confirmTitle}
    message={confirmMessage}
    confirmLabel="Delete"
    error={confirmError}
    onConfirm={executeConfirm}
    onCancel={cancelConfirm}
  />
{/if}

<div class="trash-view">
  <div class="trash-header">
    <h2>Trash</h2>
    <p class="subtitle">Deleted notes are kept for 30 days before being permanently removed.</p>
    
    {#if trashItems.length > 0}
      <button class="empty-trash-btn" onclick={handleEmptyTrash}>
        Empty Trash
      </button>
    {/if}
  </div>
  
  <div class="trash-content">
    {#if loading}
      <div class="loading">
        <div class="loading-spinner"></div>
        <span>Loading trash...</span>
      </div>
    {:else if trashItems.length === 0}
      <div class="empty-state">
        
        <h3>Trash is empty</h3>
        <p>Deleted notes will appear here and can be restored within 30 days.</p>
      </div>
    {:else}
      <div class="trash-list">
        {#each trashItems as item (item.id)}
          {@const daysRemaining = getDaysRemaining(item.deleted_at)}
          {@const isExpiringSoon = daysRemaining <= 5}
          <div class="trash-item" class:expiring={isExpiringSoon}>
            <div class="item-info">
              <div class="item-title">{item.title || item.slug}</div>
              <div class="item-meta">
                <span class="item-folder">{getOriginalFolder(item)}</span>
                <span class="item-separator">•</span>
                <span class="item-date">Deleted {formatDate(item.deleted_at)}</span>
                <span class="item-separator">•</span>
                <span class="days-remaining" class:warning={isExpiringSoon}>
                  {daysRemaining} day{daysRemaining !== 1 ? 's' : ''} remaining
                </span>
              </div>
            </div>
            <div class="item-actions">
              <button 
                class="restore-btn"
                onclick={() => handleRestore(item)}
                title="Restore note to its original location"
              >
                Restore
              </button>
              <button 
                class="delete-btn"
                onclick={() => handleDeletePermanently(item)}
                title="Permanently delete this note"
              >
                Delete Permanently
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .trash-view {
    flex: 1;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
    color: #e0e0e0;
    height: 100%;
  }

  .trash-header {
    padding: 24px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    display: flex;
    flex-direction: column;
    gap: 8px;
    position: relative;
  }

  .trash-header h2 {
    font-size: 24px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0;
  }

  .subtitle {
    font-size: 14px;
    color: #888888;
    margin: 0;
    line-height: 1.4;
  }

  .empty-trash-btn {
    position: absolute;
    top: 24px;
    right: 24px;
    padding: 8px 16px;
    border: 1px solid #EF4444;
    border-radius: 6px;
    background: transparent;
    color: #EF4444;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    font-family: inherit;
  }

  .empty-trash-btn:hover {
    background: rgba(239, 68, 68, 0.1);
    border-color: #F87171;
  }

  .trash-content {
    flex: 1;
    overflow-y: auto;
    padding: 0;
  }

  .loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 80px 20px;
    gap: 16px;
    color: #888888;
  }

  .loading-spinner {
    width: 24px;
    height: 24px;
    border: 2px solid rgba(0, 228, 228, 0.2);
    border-top: 2px solid #00E4E4;
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 80px 20px;
    text-align: center;
  }

  .empty-icon {
    font-size: 48px;
    margin-bottom: 16px;
    opacity: 0.5;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 18px;
    color: #e0e0e0;
  }

  .empty-state p {
    margin: 0;
    color: #888888;
    font-size: 14px;
    max-width: 400px;
    line-height: 1.5;
  }

  .trash-list {
    display: flex;
    flex-direction: column;
  }

  .trash-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 24px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    background: transparent;
    transition: all 0.15s ease;
  }

  .trash-item:hover {
    background: rgba(255,255,255,0.05);
  }

  .trash-item.expiring {
    border-left: 3px solid #EF4444;
  }

  .item-info {
    flex: 1;
    min-width: 0;
  }

  .item-title {
    font-size: 16px;
    font-weight: 500;
    color: #e0e0e0;
    margin-bottom: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .item-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: #888888;
    flex-wrap: wrap;
  }

  .item-folder {
    font-weight: 500;
    color: #00E4E4;
  }

  .item-separator {
    opacity: 0.5;
    font-size: 12px;
  }

  .days-remaining.warning {
    color: #EF4444;
    font-weight: 600;
  }

  .item-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
    margin-left: 16px;
  }

  .restore-btn {
    padding: 8px 16px;
    border: 1px solid #00E4E4;
    border-radius: 6px;
    background: transparent;
    color: #00E4E4;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    font-family: inherit;
  }

  .restore-btn:hover {
    background: #00E4E4;
    color: #0a0a0f;
  }

  .delete-btn {
    padding: 8px 16px;
    border: 1px solid #666666;
    border-radius: 6px;
    background: transparent;
    color: #888888;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    font-family: inherit;
  }

  .delete-btn:hover {
    border-color: #EF4444;
    color: #EF4444;
    background: rgba(239, 68, 68, 0.1);
  }

  /* Responsive design */
  @media (max-width: 768px) {
    .trash-item {
      flex-direction: column;
      align-items: flex-start;
      gap: 12px;
    }

    .item-actions {
      margin-left: 0;
      width: 100%;
      justify-content: flex-end;
    }

    .empty-trash-btn {
      position: static;
      align-self: flex-start;
      margin-top: 16px;
    }
  }
</style>