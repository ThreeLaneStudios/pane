<script lang="ts">
  // #2723 PR B — window-first updating screen.
  //
  // This component is the frontend's one gate before the real app mounts /
  // any `DbState`-dependent command fires. It wraps `+layout.svelte`'s
  // `children` and only renders them once the backend reports `ready`.
  //
  // Race-safety design (see the backend's `startup.rs` doc comments for the
  // matching half): the INITIAL fast-path/slow-path decision is read via a
  // request/response command (`get_startup_status`), not an event — an
  // event emitted before this component's `listen()` calls attach would be
  // silently lost, but a command always reflects the state at the moment
  // it's called, so there is no "missed the emit" failure mode for that
  // first read. Once in the `migration_required` state, `backup://progress`
  // / `migration://progress` / `startup://ready` / `startup://failed`
  // *events* are used for live updates (events are a good fit once we know
  // we're already inside the narrow, longer-lived migrating-launch window),
  // but a periodic re-poll of `get_startup_status` runs alongside them as a
  // defensive net in case the terminal `ready`/`failed` event itself was
  // missed (e.g. a very fast migration finishing before `listen()` attached).
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { exit } from '@tauri-apps/plugin-process';
  import { onMount, onDestroy } from 'svelte';

  let { children } = $props();

  type Phase = 'not_yet_determined' | 'migration_required' | 'ready' | 'failed';

  interface StartupStatusPayload {
    phase: Phase;
    message: string | null;
  }

  let phase: Phase = $state('not_yet_determined');
  let failureMessage: string | null = $state(null);
  let progressMessage = $state('Preparing update…');
  let progressPct: number | null = $state(null);

  let unlistenFns: UnlistenFn[] = [];
  let pollHandle: ReturnType<typeof setInterval> | null = null;

  function applyStatus(status: StartupStatusPayload) {
    phase = status.phase;
    if (status.phase === 'failed') {
      failureMessage = status.message ?? 'An unknown error occurred.';
    }
  }

  async function pollStatusOnce() {
    try {
      const status = await invoke<StartupStatusPayload>('get_startup_status');
      applyStatus(status);
    } catch (e) {
      // get_startup_status has no DbState dependency and should never
      // throw; if it somehow does, don't strand the user on a silent
      // blank screen — surface it the same way a real startup failure
      // would be surfaced (SOUL.md §4: no error swallowed silently).
      console.error('[UpdatingScreen] get_startup_status failed:', e);
      failureMessage = `Could not determine startup status: ${e}`;
      phase = 'failed';
    }
  }

  function stopPolling() {
    if (pollHandle !== null) {
      clearInterval(pollHandle);
      pollHandle = null;
    }
  }

  function quit() {
    exit(1);
  }

  onMount(() => {
    (async () => {
      await pollStatusOnce();

      if (phase !== 'migration_required') {
        // Fast path (the overwhelming majority of launches) or an
        // already-resolved slow path — nothing further to listen for.
        return;
      }

      // Defensive re-poll net (see file-level comment) — cleared the
      // moment we leave migration_required.
      pollHandle = setInterval(() => {
        if (phase === 'migration_required') {
          pollStatusOnce();
        } else {
          stopPolling();
        }
      }, 500);

      unlistenFns.push(
        await listen<{ phase: string; message: string; pct: number | null }>(
          'backup://progress',
          (event) => {
            progressMessage = event.payload.message;
            if (typeof event.payload.pct === 'number') {
              progressPct = event.payload.pct;
            }
          }
        )
      );

      unlistenFns.push(
        await listen<{ step: number; total: number; label: string; pct: number }>(
          'migration://progress',
          (event) => {
            progressMessage = `Migrating database (step ${event.payload.step} of ${event.payload.total})`;
            progressPct = event.payload.pct;
          }
        )
      );

      unlistenFns.push(
        await listen('startup://ready', () => {
          stopPolling();
          phase = 'ready';
        })
      );

      unlistenFns.push(
        await listen<{ message: string }>('startup://failed', (event) => {
          stopPolling();
          failureMessage = event.payload.message;
          phase = 'failed';
        })
      );
    })();
  });

  onDestroy(() => {
    stopPolling();
    for (const fn of unlistenFns) fn();
  });
</script>

{#if phase === 'ready'}
  {@render children()}
{:else if phase === 'failed'}
  <div class="updating-overlay">
    <div class="updating-card">
      <div class="updating-title failed">Update Failed</div>
      <div class="updating-message">{failureMessage}</div>
      <div class="updating-reassurance">
        Pane takes a backup of your database before applying updates — your
        data is safe. Quit and relaunch Pane to try again.
      </div>
      <button class="btn-quit" onclick={quit}>Quit Pane</button>
    </div>
  </div>
{:else if phase === 'migration_required'}
  <div class="updating-overlay">
    <div class="updating-card">
      <div class="updating-title">Updating Pane…</div>
      <div class="updating-subtitle">Don't quit</div>
      <div class="progress-track">
        <div
          class="progress-fill"
          style:width={progressPct !== null ? `${progressPct}%` : '15%'}
        ></div>
      </div>
      <div class="updating-message">{progressMessage}</div>
    </div>
  </div>
{:else}
  <!-- not_yet_determined: a vanishingly brief instant on every launch (see
       startup.rs) — a neutral, content-free screen rather than either the
       real app (which might fire a DbState-dependent command too early) or
       the "Updating Pane" copy (we don't yet know a migration is even
       happening). -->
  <div class="updating-overlay minimal"></div>
{/if}

<style>
  .updating-overlay {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    z-index: 9000;
    background: #0a0a0f;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .updating-overlay.minimal {
    background: #0a0a0f;
  }
  .updating-card {
    text-align: center;
    max-width: 380px;
    width: 90%;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .updating-title {
    font-size: 18px;
    font-weight: 700;
    color: #e0e0e0;
    margin-bottom: 4px;
  }
  .updating-title.failed {
    color: #ff6b6b;
  }
  .updating-subtitle {
    font-size: 13px;
    color: #00E4E4;
    margin-bottom: 20px;
  }
  .progress-track {
    width: 100%;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
    margin-bottom: 12px;
  }
  .progress-fill {
    height: 100%;
    background: #00E4E4;
    transition: width 0.3s ease;
  }
  .updating-message {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.6);
    margin-bottom: 16px;
  }
  .updating-reassurance {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.45);
    margin-bottom: 20px;
  }
  .btn-quit {
    background: #ff6b6b;
    color: #0a0a0f;
    border: none;
    border-radius: 4px;
    padding: 8px 24px;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
    font-family: inherit;
  }
  .btn-quit:hover {
    background: #ff8585;
  }
</style>
