<script lang="ts">
  import type { NoteMeta } from '$lib/types';

  interface Props {
    note?: NoteMeta;
  }

  let { note }: Props = $props();

  function formatWordCount(count?: number): string {
    if (!count) return '0 words';
    if (count === 1) return '1 word';
    return `${count.toLocaleString()} words`;
  }

  function formatReadingTime(minutes?: number): string {
    if (!minutes) return '0 min read';
    if (minutes === 1) return '1 min read';
    return `${minutes} min read`;
  }

  function formatDate(dateString?: string): string {
    if (!dateString) return '';
    
    try {
      const date = new Date(dateString);
      const now = new Date();
      const diffMs = now.getTime() - date.getTime();
      const diffMinutes = Math.floor(diffMs / (1000 * 60));
      const diffHours = Math.floor(diffMinutes / 60);
      const diffDays = Math.floor(diffHours / 24);

      if (diffMinutes < 1) return 'just now';
      if (diffMinutes < 60) return `${diffMinutes}m ago`;
      if (diffHours < 24) return `${diffHours}h ago`;
      if (diffDays < 7) return `${diffDays}d ago`;
      
      return date.toLocaleDateString();
    } catch {
      return dateString;
    }
  }
</script>

<div class="status-bar">
  <div class="status-left">
    {#if note}
      <span class="status-item">
        <span class="status-value">{formatWordCount(note.word_count)}</span>
      </span>
      <span class="status-separator">•</span>
      <span class="status-item">
        <span class="status-value">{formatReadingTime(note.reading_time)}</span>
      </span>
    {/if}
  </div>
  
  <div class="status-right">
    {#if note}
      <span class="status-item">
        <span class="status-value">{formatDate(note.updated_at)}</span>
      </span>

    {/if}
  </div>
</div>

<style>
  .status-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 16px;
    background: #0F0F11;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    font-size: 11px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    color: #666666;
    min-height: 28px;
  }

  .status-left,
  .status-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-item {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .status-item.pinned-legacy {
    color: #00E4E4;
  }

  .status-value {
    color: #666666;
    font-weight: 400;
  }

  .status-separator {
    opacity: 0.5;
    color: #666666;
  }

  /* Responsive design */
  @media (max-width: 768px) {
    .status-bar {
      flex-direction: column;
      align-items: flex-start;
      gap: 4px;
      padding: 6px 16px;
    }

    .status-left,
    .status-right {
      gap: 6px;
    }

    .status-item {
      font-size: 10px;
    }
  }
</style>