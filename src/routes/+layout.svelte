<script lang="ts">
  import '../app.css';
  import UpdateBar from '$lib/components/UpdateBar.svelte';
  // #2723 PR B: gates mounting the real app behind the backend's startup
  // status (fast path: resolves to "ready" effectively instantly, no
  // visible change; migrating launch: shows "Updating Pane… don't quit"
  // with real progress until the background migration finishes).
  import UpdatingScreen from '$lib/components/UpdatingScreen.svelte';
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  
  let { children } = $props();
  
  let syncFailureMessage = $state('');
  let showSyncFailure = $state(false);
  
  onMount(() => {
    // Listen for sync failure events
    const unlisten = listen('sync-failure', (event: any) => {
      const payload = event.payload;
      syncFailureMessage = `Sync failed for ${payload.filename}. Will retry on next edit.`;
      showSyncFailure = true;
      
      // Auto-hide after 10 seconds
      setTimeout(() => {
        showSyncFailure = false;
      }, 10000);
    });
    
    return () => {
      unlisten.then(fn => fn());
    };
  });
</script>

{#if showSyncFailure}
  <div class="sync-failure-banner">
    ⚠️ {syncFailureMessage}
  </div>
{/if}

<UpdateBar />
<UpdatingScreen>
  {@render children()}
</UpdatingScreen>

<style>
  .sync-failure-banner {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    background: #ff6b6b;
    color: white;
    padding: 12px;
    text-align: center;
    z-index: 10000;
    font-size: 14px;
    font-weight: 500;
  }
</style>
