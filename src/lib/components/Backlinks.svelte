<script lang="ts">
  import * as api from '$lib/api';

  interface Props {
    noteId?: string;
    onNavigate?: (title: string) => void;
  }

  let { noteId, onNavigate }: Props = $props();

  let backlinks = $state<Array<{ id: string; title: string; project: string }>>([]);
  let loading = $state(false);

  // Load backlinks when noteId changes
  $effect(() => {
    if (noteId) {
      loadBacklinks();
    } else {
      backlinks = [];
    }
  });

  async function loadBacklinks() {
    if (!noteId) return;
    
    loading = true;
    try {
      backlinks = await api.getBacklinks(noteId);
    } catch (error) {
      console.error('Failed to load backlinks:', error);
      backlinks = [];
    } finally {
      loading = false;
    }
  }

  function handleBacklinkClick(title: string) {
    if (onNavigate) {
      onNavigate(title);
    }
  }
</script>

{#if backlinks.length > 0}
  <div class="backlinks">
    <h3 class="backlinks-title">LINKED FROM</h3>
    <div class="backlinks-list">
      {#each backlinks as backlink}
        <button
          class="backlink-item"
          onclick={() => handleBacklinkClick(backlink.title)}
        >
          <span class="backlink-title">{backlink.title}</span>
          <span class="backlink-project">{backlink.project}</span>
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .backlinks {
    margin-top: 32px;
    padding-top: 24px;
    border-top: 1px solid var(--border-color);
  }

  .backlinks-title {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-tertiary);
    margin: 0 0 12px 0;
  }

  .backlinks-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .backlink-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border: none;
    background: transparent;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
    transition: all 0.15s ease;
    font-family: inherit;
    width: 100%;
  }

  .backlink-item:hover {
    background: var(--bg-hover);
  }

  .backlink-title {
    font-size: 14px;
    color: var(--text-primary);
    font-weight: 500;
  }

  .backlink-project {
    font-size: 12px;
    color: var(--text-secondary);
  }
</style>