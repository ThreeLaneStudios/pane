<script lang="ts">
  import { check, type Update } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';

  let updateAvailable: Update | null = $state(null);
  let updating = $state(false);
  let dismissed = $state(false);

  async function checkForUpdate() {
    try {
      const update = await check();
      if (update) {
        updateAvailable = update;
        dismissed = false;
      }
    } catch (e) {
      console.error('Update check failed:', e);
    }
  }

  async function installUpdate() {
    if (!updateAvailable) return;
    updating = true;
    try {
      await updateAvailable.downloadAndInstall();
      await relaunch();
    } catch (e) {
      console.error('Update failed:', e);
      updating = false;
    }
  }

  function dismiss() { dismissed = true; }

  // Auto-update disabled until paneapp.ai update infrastructure is ready (#284)
  // $effect(() => {
  //   checkForUpdate();
  //   const interval = setInterval(checkForUpdate, 4 * 60 * 60 * 1000);
  //   return () => clearInterval(interval);
  // });
</script>

{#if updateAvailable && !dismissed}
  <div class="update-overlay">
    <div class="update-modal">
      <div class="update-title">Update Available</div>
      <div class="update-version">v{updateAvailable.version}</div>
      <div class="update-actions">
        {#if updating}
          <div class="update-installing">Installing...</div>
        {:else}
          <button class="btn-cancel" onclick={dismiss}>Later</button>
          <button class="btn-update" onclick={installUpdate}>Update Now</button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .update-overlay {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    z-index: 200;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .update-modal {
    background: #0a0a0f;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 32px;
    max-width: 340px;
    width: 90%;
    text-align: center;
  }
  .update-title {
    font-size: 16px;
    font-weight: 600;
    color: #e0e0e0;
    margin-bottom: 4px;
  }
  .update-version {
    font-size: 13px;
    color: #00E4E4;
    margin-bottom: 20px;
  }
  .update-actions {
    display: flex;
    gap: 10px;
    justify-content: center;
  }
  .btn-cancel {
    background: rgba(255, 255, 255, 0.05);
    color: rgba(255, 255, 255, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    padding: 8px 20px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .btn-cancel:hover { color: #e0e0e0; border-color: rgba(255, 255, 255, 0.15); }
  .btn-update {
    background: #00E4E4;
    color: #0a0a0f;
    border: none;
    border-radius: 4px;
    padding: 8px 20px;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .btn-update:hover { background: #00d0d0; }
  .update-installing { font-size: 13px; color: #00E4E4; }
</style>
