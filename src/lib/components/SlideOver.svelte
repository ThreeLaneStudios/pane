<script lang="ts">
  import type { NoteMeta } from '$lib/types';
  import TaskMetadataHeader from './TaskMetadataHeader.svelte';
  import * as noteService from '$lib/services/noteService';
  import { renderMarkdown } from '$lib/utils/markdown';

  interface Props {
    note: NoteMeta;
    onClose: () => void;
    onUpdate?: (updatedNote: NoteMeta) => void;
  }

  let { note, onClose, onUpdate }: Props = $props();

  let noteContent = $state('');
  let isLoading = $state(true);
  let isVisible = $state(false);

  // Animation lifecycle
  $effect(() => {
    // Slide in animation
    setTimeout(() => {
      isVisible = true;
    }, 10);

    return () => {
      isVisible = false;
    };
  });

  // Load note content
  $effect(() => {
    if (note) {
      isLoading = true;
      (async () => {
        try {
          const fullNote = await noteService.loadNoteWithJson(note.notebook, note.slug);
          noteContent = fullNote?.content || '';
        } catch (error) {
          console.error('Failed to load note content:', error);
          noteContent = 'Failed to load content';
        } finally {
          isLoading = false;
        }
      })();
    }
  });

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      handleClose();
    }
  }

  function handleClose() {
    isVisible = false;
    // Wait for slide-out animation to complete
    setTimeout(() => {
      onClose();
    }, 300);
  }

  function handleEscapeKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      handleClose();
    }
  }

  function formatDate(dateStr: string): string {
    if (!dateStr) return '';
    try {
      return new Date(dateStr).toLocaleDateString([], {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      });
    } catch {
      return '';
    }
  }

  function getPriorityLabel(priority: string | undefined): string {
    if (!priority) return '';
    return priority.charAt(0).toUpperCase() + priority.slice(1);
  }

  function getPriorityColor(priority: string | undefined): string {
    switch (priority) {
      case 'urgent': return '#EF4444';
      case 'high': return '#FB923C';
      case 'low': return '#666666';
      default: return '#00E4E4';
    }
  }

  function getStatusLabel(status: string | undefined): string {
    switch (status) {
      case 'cabinet': return 'Backlog';
      case 'todo': return 'Todo';
      case 'in_progress': return 'In Progress';
      case 'done': return 'Done';
      default: return '';
    }
  }

  function getStatusColor(status: string | undefined): string {
    switch (status) {
      case 'cabinet': return '#6B7280';
      case 'todo': return '#60A5FA';
      case 'in_progress': return '#00E4E4';
      case 'done': return '#10B981';
      default: return '#6B7280';
    }
  }

</script>

<svelte:window onkeydown={handleEscapeKey} />

<!-- Backdrop -->
<div 
  class="slide-over-backdrop" 
  class:visible={isVisible}
  onclick={handleBackdropClick}
  role="dialog"
  aria-modal="true"
  aria-label="Details"
>
  <!-- Slide-over Panel -->
  <div class="slide-over-panel" class:visible={isVisible}>
    <!-- Header -->
    <div class="slide-over-header">
      <div class="header-title">
        <h3>{note.title || 'Untitled'}</h3>
        <button class="close-btn" onclick={handleClose} title="Close">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>
      
      <!-- Task Metadata (if note has task status) -->
      {#if note.task_status}
        <div class="task-metadata">
          <div class="metadata-row">
            <div class="metadata-item">
              <span class="metadata-label">Status</span>
              <div class="status-badge" style:color={getStatusColor(note.task_status)}>
                {getStatusLabel(note.task_status)}
              </div>
            </div>
            
            {#if note.task_priority}
              <div class="metadata-item">
                <span class="metadata-label">Priority</span>
                <div class="priority-badge" style:color={getPriorityColor(note.task_priority)}>
                  <div class="priority-dot" style:background-color={getPriorityColor(note.task_priority)}></div>
                  {getPriorityLabel(note.task_priority)}
                </div>
              </div>
            {/if}
          </div>

          {#if note.task_due_date || note.task_assignee}
            <div class="metadata-row">
              {#if note.task_due_date}
                <div class="metadata-item">
                  <span class="metadata-label">Due Date</span>
                  <span class="metadata-value">{formatDate(note.task_due_date)}</span>
                </div>
              {/if}
              
              {#if note.task_assignee}
                <div class="metadata-item">
                  <span class="metadata-label">Assignee</span>
                  <span class="metadata-value">{note.task_assignee}</span>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Content -->
    <div class="slide-over-content">
      {#if isLoading}
        <div class="loading-state">
          <div class="loading-spinner"></div>
          <p>Loading content...</p>
        </div>
      {:else}
        <div class="note-content">
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          {@html renderMarkdown(noteContent)}
        </div>
      {/if}
    </div>

    <!-- Footer with note info -->
    <div class="slide-over-footer">
      <div class="note-meta">
        {#if note.notebook.includes('/')}
          <span class="project-folder">
            {note.notebook.replace('/', ' / ')}
          </span>
        {:else}
          <span class="project-name">{note.notebook}</span>
        {/if}
        <span class="separator">•</span>
        <span class="update-time">Updated {formatDate(note.updated_at)}</span>
      </div>
    </div>
  </div>
</div>

<style>
  .slide-over-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.5);
    opacity: 0;
    visibility: hidden;
    transition: all 0.3s ease;
    z-index: 1000;
  }

  .slide-over-backdrop.visible {
    opacity: 1;
    visibility: visible;
  }

  .slide-over-panel {
    position: fixed;
    top: 0;
    right: 0;
    width: 500px;
    height: 100vh;
    background: rgba(255,255,255,0.05);
    border-left: 1px solid rgba(255,255,255,0.06);
    display: flex;
    flex-direction: column;
    transform: translateX(100%);
    transition: transform 0.3s ease;
    box-shadow: -4px 0 24px rgba(0, 0, 0, 0.2);
  }

  .slide-over-panel.visible {
    transform: translateX(0);
  }

  .slide-over-header {
    flex-shrink: 0;
    padding: 20px 24px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }

  .header-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .header-title h3 {
    font-size: 18px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: #888888;
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
    margin-left: 16px;
  }

  .close-btn:hover {
    background: rgba(255,255,255,0.06);
    color: #e0e0e0;
  }

  .task-metadata {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .metadata-row {
    display: flex;
    gap: 20px;
  }

  .metadata-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }

  .metadata-label {
    font-size: 11px;
    color: #666666;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .metadata-value {
    font-size: 14px;
    color: #e0e0e0;
    font-weight: 500;
  }

  .status-badge {
    font-size: 14px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .priority-badge {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
    font-weight: 500;
  }

  .priority-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .slide-over-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    color: #888888;
    height: 200px;
  }

  .loading-spinner {
    width: 24px;
    height: 24px;
    border: 2px solid rgba(0,228,228,0.1);
    border-top: 2px solid #00E4E4;
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    0% { transform: rotate(0deg); }
    100% { transform: rotate(360deg); }
  }

  .note-content {
    color: #e0e0e0;
    line-height: 1.6;
    font-size: 15px;
  }

  .note-content :global(h1) {
    font-size: 24px;
    font-weight: 700;
    color: #e0e0e0;
    margin: 0 0 16px 0;
  }

  .note-content :global(h2) {
    font-size: 20px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 24px 0 12px 0;
  }

  .note-content :global(h3) {
    font-size: 16px;
    font-weight: 600;
    color: #888888;
    margin: 20px 0 8px 0;
  }

  .note-content :global(p) {
    margin: 0 0 16px 0;
  }

  .note-content :global(strong) {
    font-weight: 600;
    color: #e0e0e0;
  }

  .note-content :global(em) {
    font-style: italic;
    color: #888888;
  }

  .note-content :global(br) {
    line-height: 1.2;
  }

  .slide-over-footer {
    flex-shrink: 0;
    padding: 16px 24px;
    border-top: 1px solid rgba(255,255,255,0.06);
  }

  .note-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: #666666;
  }

  .project-folder {
    color: #00E4E4;
    font-weight: 500;
  }

  .project-name {
    color: #888888;
    font-weight: 500;
  }

  .separator {
    color: #666666;
  }

  .update-time {
    color: #666666;
  }
</style>