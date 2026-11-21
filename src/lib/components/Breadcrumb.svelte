<script lang="ts">
  import { activeProject, activeFolder, activeView } from '$lib/stores/app';
  import type { Project, Folder } from '$lib/types';

  interface Props {
    onNavigateToProject?: (project: Project) => void;
  }

  let { onNavigateToProject }: Props = $props();

  function handleProjectClick() {
    if ($activeProject && onNavigateToProject) {
      onNavigateToProject($activeProject);
    }
  }
</script>

<!-- Only show breadcrumb at folder level (not project level) -->
{#if $activeFolder && $activeProject && $activeView === 'folder'}
  <nav class="breadcrumb" aria-label="Page breadcrumb">
    <button 
      class="breadcrumb-link" 
      onclick={handleProjectClick}
      title="Go to {$activeProject.name} overview"
    >
      {$activeProject.name}
    </button>
    <span class="breadcrumb-separator">›</span>
    <span class="breadcrumb-current">{$activeFolder.name}</span>
  </nav>
{/if}

<style>
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    background: rgba(255,255,255,0.05);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    font-size: 12px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    min-height: 32px;
  }

  .breadcrumb-link {
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    font-size: inherit;
    font-family: inherit;
    padding: 0;
    transition: color 0.15s ease;
  }

  .breadcrumb-link:hover {
    color: #e0e0e0;
  }

  .breadcrumb-separator {
    color: #666666;
    font-size: 12px;
  }

  .breadcrumb-current {
    color: #888888;
    font-weight: 500;
  }
</style>