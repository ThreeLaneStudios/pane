<script lang="ts">
  /**
   * NavPlayer.svelte — Global nav-docked audio player (PR5a skeleton, #401 / #1801).
   *
   * Owns the <audio> element. Reads playerTrack, playerStatus, playerPosition,
   * playerDuration from $lib/stores/player.
   *
   * States: idle (never shown) | loading | playing | paused | failed
   *
   * Width is CSS-driven via .sidebar:hover cascade — no JS state for width.
   * Collapsed (40px): 34px play/pause button + cyan SVG progress ring.
   * Expanded (220px on hover): full transport with cover, title, breadcrumb,
   *   scrubber, elapsed/total, and go-to-note (↗).
   *
   * NodeView wiring (AttachmentNodeView → playerTrack) is PR5b.
   */

  import { onDestroy } from 'svelte';
  import { get } from 'svelte/store';

  import {
    playerTrack,
    playerStatus,
    playerPosition,
    playerDuration,
  } from '$lib/stores/player';
  import { activeNote } from '$lib/stores/app';
  import * as noteService from '$lib/services/noteService';
  import { readNote } from '$lib/api';
  import { navigateTo } from '$lib/services/navigation';

  // ─── Audio element ref ─────────────────────────────────────────────────────
  let audio: HTMLAudioElement | null = $state(null);

  // ─── Go-to-note IPC guard ──────────────────────────────────────────────────
  let ipcInFlight = $state(false);

  // ─── Error toast (#1876-fix) ───────────────────────────────────────────────
  let errorToastMsg = $state<string | null>(null);
  let errorToastTimer: ReturnType<typeof setTimeout> | null = null;

  function showErrorToast(msg: string) {
    errorToastMsg = msg;
    if (errorToastTimer) clearTimeout(errorToastTimer);
    errorToastTimer = setTimeout(() => { errorToastMsg = null; }, 5000);
  }

  // ─── Derived local state ───────────────────────────────────────────────────
  let track = $derived($playerTrack);
  let status = $derived($playerStatus);
  let position = $derived($playerPosition);
  let duration = $derived($playerDuration);

  // Computed scrubber fill percentage
  let scrubberPct = $derived(
    duration > 0 ? Math.min(100, (position / duration) * 100) : 0
  );

  // Computed SVG ring for collapsed state
  // 34px button, ring drawn on 32px circle (r=14, cx/cy=17)
  const RING_CIRCUMFERENCE = 2 * Math.PI * 14; // ≈ 87.96
  let ringDash = $derived(
    duration > 0
      ? (position / duration) * RING_CIRCUMFERENCE
      : 0
  );

  // Time formatting
  function fmt(secs: number): string {
    if (!isFinite(secs) || secs < 0) return '0:00';
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, '0')}`;
  }

  // ─── Trigger .load() on mount and on track change ──────────────────────────
  // WKWebView on macOS does not reliably auto-fetch asset:// URIs for <audio>
  // elements created with src pre-populated at mount time. The HTML spec resource
  // selection algorithm fires when src changes on an *already-attached* element
  // (which AttachmentVideoPlayer exploits correctly), but NOT reliably when the
  // element is created-with-src in WKWebView's asset:// scheme handler.
  //
  // Solution: mirror the pattern used in retry() — call audio.load() explicitly
  // after bind:this resolves. Keyed on track?.src so a track swap re-fires load
  // without requiring NavPlayer to unmount/remount. Guard: skip if src is empty.
  $effect(() => {
    const src = track?.src;
    if (!audio || !src) return;
    audio.load();
  });

  // ─── Reactive: cleanup when track goes null ─────────────────────────────────
  // We do NOT auto-set 'loading' here — the caller sets it when they set the
  // track (e.g. AttachmentNodeView in PR5b, or setInterval-based tests).
  // The audio element's own event handlers drive status transitions:
  //   loadstart   → (caller already set 'loading')
  //   canplay     → 'playing'
  //   error       → 'failed'
  // This keeps the component testable: tests can set status independently.
  $effect(() => {
    const t = $playerTrack;
    if (!t) {
      // Track cleared — reset everything
      playerStatus.set('idle');
      playerPosition.set(0);
      playerDuration.set(0);
    }
  });

  // ─── Audio event handlers ──────────────────────────────────────────────────

  function onCanPlay() {
    // Transition loading → playing and auto-play.
    // The caller is responsible for setting status='loading' when setting a track.
    if ($playerStatus === 'loading') {
      playerStatus.set('playing');
      audio?.play().catch(() => {
        // Autoplay blocked — treat as paused
        playerStatus.set('paused');
      });
    }
  }

  function onDurationChange() {
    if (audio) {
      playerDuration.set(isFinite(audio.duration) ? audio.duration : 0);
    }
  }

  function onTimeUpdate() {
    if (audio) {
      playerPosition.set(audio.currentTime);
    }
  }

  function onAudioError() {
    playerStatus.set('failed');
  }

  function onEnded() {
    playerStatus.set('paused');
    playerPosition.set(0);
    if (audio) audio.currentTime = 0;
  }

  // ─── Controls ──────────────────────────────────────────────────────────────

  function togglePlayPause() {
    if (!audio) return;
    if ($playerStatus === 'playing') {
      audio.pause();
      playerStatus.set('paused');
    } else if ($playerStatus === 'paused') {
      audio.play().catch(() => {});
      playerStatus.set('playing');
    }
  }

  function seek(e: MouseEvent) {
    if (!audio || duration <= 0) return;
    const bar = e.currentTarget as HTMLElement;
    const rect = bar.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    audio.currentTime = ratio * duration;
    playerPosition.set(audio.currentTime);
  }

  function retry() {
    if (!audio) return;
    playerStatus.set('loading');
    playerPosition.set(0);
    audio.load();
  }

  async function handleGoToNote() {
    const track = get(playerTrack);
    const id = track?.noteId;
    if (!id) return;

    // Same-note guard: no-op (scroll-to-attachment deferred to #1875)
    if (get(activeNote)?.meta?.id === id) return;

    try {
      ipcInFlight = true;
      // Direct DB lookup via IPC — bypasses the notebook-scoped notes store
      // which evicts cross-project notes on navigation (noteService.ts:115).
      const noteContent = await readNote('', id);
      await navigateTo({ view: 'notes_browser', projectId: noteContent.meta.project_id });
      await noteService.selectNote(noteContent.meta);
    } catch (e) {
      console.error('[NavPlayer] Failed to open source note:', e);
      showErrorToast('Could not open source note');
    } finally {
      ipcInFlight = false;
    }
  }

  // ─── Cleanup ───────────────────────────────────────────────────────────────
  onDestroy(() => {
    if (audio) {
      audio.pause();
      audio.src = '';
    }
  });
</script>

{#if track !== null}
  <!-- Hidden audio element — always in DOM when a track is loaded -->
  <audio
    bind:this={audio}
    src={track.src}
    oncanplay={onCanPlay}
    ondurationchange={onDurationChange}
    ontimeupdate={onTimeUpdate}
    onerror={onAudioError}
    onended={onEnded}
    preload="metadata"
  ></audio>

  {#if status === 'failed'}
    <!-- ── FAILED STATE ── -->
    <div class="np-block np-block--failed">
      <!-- Collapsed: minimal red ring -->
      <div class="np-collapsed-only">
        <div class="np-ring-btn np-ring-btn--failed" title="Playback failed">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4M12 17h.01"/></svg>
        </div>
      </div>

      <!-- Expanded: full failed card -->
      <div class="np-expanded-only np-failed-card">
        <div class="np-failed-icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4M12 17h.01"/></svg>
        </div>
        <div class="np-failed-body">
          <div class="np-failed-title">Couldn't play this file</div>
          <div class="np-failed-sub">{track.title}</div>
        </div>
        <button class="np-retry-btn" onclick={retry} title="Retry">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/></svg>
          Retry
        </button>
      </div>
    </div>

  {:else}
    <!-- ── IDLE / LOADING / PLAYING / PAUSED STATES ── -->
    <div class="np-block" class:np-block--loading={status === 'loading'}>

      <!-- ── COLLAPSED VIEW (40px sidebar) ── -->
      <div class="np-collapsed-only">
        <div class="np-ring-wrap" role="button" tabindex="0"
          onclick={togglePlayPause}
          onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); togglePlayPause(); } }}
          title={status === 'playing' ? 'Pause' : 'Play'}
          aria-label={status === 'playing' ? 'Pause' : 'Play'}
        >
          <!-- SVG progress ring -->
          <svg class="np-ring-svg" width="34" height="34" viewBox="0 0 34 34">
            <!-- Track -->
            <circle class="np-ring-track" cx="17" cy="17" r="14" fill="none" stroke-width="2.5" />
            <!-- Fill — rotated -90° so it starts at top -->
            {#if status !== 'loading'}
              <circle
                class="np-ring-fill"
                cx="17" cy="17" r="14"
                fill="none"
                stroke-width="2.5"
                stroke-dasharray="{ringDash} {RING_CIRCUMFERENCE}"
                transform="rotate(-90 17 17)"
              />
            {:else}
              <!-- Loading spinner arc -->
              <circle
                class="np-ring-spin"
                cx="17" cy="17" r="14"
                fill="none"
                stroke-width="2.5"
                stroke-dasharray="30 {RING_CIRCUMFERENCE - 30}"
                transform="rotate(-90 17 17)"
              />
            {/if}
          </svg>
          <!-- Play/Pause icon centered in ring -->
          <div class="np-ring-icon">
            {#if status === 'loading'}
              <div class="np-spin-dot"></div>
            {:else if status === 'playing'}
              <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" stroke="none"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>
            {:else}
              <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M7 4v16l13-8z"/></svg>
            {/if}
          </div>
        </div>
      </div>

      <!-- ── EXPANDED VIEW (220px on hover) ── -->
      <div class="np-expanded-only np-expanded">

        <!-- Row 1: Cover/EQ + title + breadcrumb + go-to-note -->
        <div class="np-header">
          <div class="np-cover" class:np-cover--loading={status === 'loading'}>
            {#if status === 'loading'}
              <div class="np-cover-spinner"></div>
            {:else}
              <!-- EQ bars animation (playing) or static (paused) -->
              <div class="np-eq" class:np-eq--animated={status === 'playing'} aria-hidden="true">
                <i></i><i></i><i></i><i></i>
              </div>
            {/if}
          </div>

          <div class="np-meta">
            <div class="np-title" title={track.title}>{track.title}</div>
            <div class="np-breadcrumb">
              {#if status === 'loading'}
                <span class="np-loading-text">buffering…</span>
              {:else}
                {track.breadcrumb}
              {/if}
            </div>
          </div>

          <button
            class="np-goto-btn"
            onclick={handleGoToNote}
            title="Go to note"
            aria-label="Go to note"
            disabled={track.noteId === null || ipcInFlight}
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3h7v7"/><path d="M21 3 11 13"/><path d="M18 14v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h5"/></svg>
          </button>
        </div>

        <!-- Row 2: Play/Pause + Scrubber + Time -->
        <div class="np-transport">
          <button
            class="np-play-btn"
            onclick={togglePlayPause}
            disabled={status === 'loading'}
            title={status === 'playing' ? 'Pause' : 'Play'}
            aria-label={status === 'playing' ? 'Pause' : 'Play'}
          >
            {#if status === 'loading'}
              <div class="np-spin-sm"></div>
            {:else if status === 'playing'}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" stroke="none"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>
            {:else}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M7 4v16l13-8z"/></svg>
            {/if}
          </button>

          <!-- Scrubber -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="np-scrubber" onclick={seek} role="slider" aria-label="Seek" aria-valuemin={0} aria-valuemax={duration} aria-valuenow={position} tabindex="0">
            <div class="np-scrubber-fill" style="width: {scrubberPct}%"></div>
          </div>

          <!-- Time -->
          <div class="np-time">
            <span class="np-time-elapsed">{fmt(position)}</span>
            <span class="np-time-sep">/</span>
            <span class="np-time-total">{fmt(duration)}</span>
          </div>
        </div>

      </div>
    </div>
  {/if}
{/if}

{#if errorToastMsg}
<div class="error-toast" role="alert">
  <span class="error-toast-msg">{errorToastMsg}</span>
  <button class="error-toast-dismiss" onclick={() => { errorToastMsg = null; }}>✕</button>
</div>
{/if}

<style>
  /* ── Layout helpers ────────────────────────────────────────────────────── */

  /* Collapsed view: visible only in narrow sidebar (default 40px) */
  .np-collapsed-only {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    flex-shrink: 0;
  }

  /* Expanded view: hidden in narrow state, shown on hover */
  .np-expanded-only {
    display: none;
  }

  /* Show expanded when sidebar is hovered (and not locked collapsed) */
  :global(.sidebar:hover) .np-expanded-only,
  :global(.sidebar.expanded) .np-expanded-only {
    display: flex;
  }

  /* Hide collapsed ring when sidebar is expanded */
  :global(.sidebar:hover) .np-collapsed-only,
  :global(.sidebar.expanded) .np-collapsed-only {
    display: none;
  }

  /* Locked-collapsed: never show expanded even on hover */
  :global(.sidebar.collapsed:hover) .np-expanded-only,
  :global(.sidebar.collapsed.expanded) .np-expanded-only {
    display: none;
  }

  :global(.sidebar.collapsed:hover) .np-collapsed-only,
  :global(.sidebar.collapsed.expanded) .np-collapsed-only {
    display: flex;
  }

  /* ── Block wrapper ─────────────────────────────────────────────────────── */
  .np-block {
    width: 100%;
    box-sizing: border-box;
  }

  /* ── Collapsed: progress ring button ──────────────────────────────────── */
  .np-ring-wrap {
    position: relative;
    width: 34px;
    height: 34px;
    cursor: pointer;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    outline: none;
  }

  .np-ring-wrap:hover .np-ring-track {
    stroke: var(--border, rgba(255,255,255,0.12));
  }

  .np-ring-svg {
    position: absolute;
    top: 0;
    left: 0;
    width: 34px;
    height: 34px;
  }

  .np-ring-track {
    stroke: var(--border-faint, rgba(255,255,255,0.06));
    transition: stroke var(--dur-fast, 0.15s) var(--ease, ease);
  }

  .np-ring-fill {
    stroke: var(--accent, #00E4E4);
    transition: stroke-dasharray var(--dur-fast, 0.15s) var(--ease, ease);
  }

  .np-ring-spin {
    stroke: var(--accent, #00E4E4);
    animation: np-ring-rotate 900ms linear infinite;
  }

  @keyframes np-ring-rotate {
    to { transform: rotate(360deg); transform-origin: 17px 17px; }
  }

  .np-ring-icon {
    position: relative;
    z-index: 1;
    color: var(--fg-1, #e0e0e0);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .np-ring-btn {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--status-danger, #ef4444);
  }

  .np-ring-btn--failed {
    background: rgba(239, 68, 68, 0.12);
  }

  /* Spinner dot for collapsed loading */
  .np-spin-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent, #00E4E4);
    animation: np-pulse 900ms ease-in-out infinite;
  }

  @keyframes np-pulse {
    0%, 100% { opacity: 0.3; }
    50% { opacity: 1; }
  }

  /* ── Expanded view ─────────────────────────────────────────────────────── */
  .np-expanded {
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    width: 220px;
    box-sizing: border-box;
  }

  /* Header row */
  .np-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }

  /* Cover / EQ tile */
  .np-cover {
    width: 32px;
    height: 32px;
    border-radius: 6px;
    background: var(--surface-2, #1e1e22);
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    position: relative;
  }

  /* EQ bars */
  .np-eq {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 16px;
    padding: 0 2px;
  }

  .np-eq i {
    display: block;
    width: 3px;
    background: var(--accent, #00E4E4);
    border-radius: 1.5px;
    height: 14px;
    flex-shrink: 0;
    transform-origin: bottom;
    will-change: transform;
  }

  .np-eq--animated i:nth-child(1) { animation: np-eq1 0.8s ease-in-out infinite; }
  .np-eq--animated i:nth-child(2) { animation: np-eq2 0.9s ease-in-out infinite; }
  .np-eq--animated i:nth-child(3) { animation: np-eq3 0.7s ease-in-out infinite; }
  .np-eq--animated i:nth-child(4) { animation: np-eq4 1.0s ease-in-out infinite; }

  @keyframes np-eq1 { 0%, 100% { transform: scaleY(0.286); } 50% { transform: scaleY(1); } }
  @keyframes np-eq2 { 0%, 100% { transform: scaleY(0.714); } 50% { transform: scaleY(0.429); } }
  @keyframes np-eq3 { 0%, 100% { transform: scaleY(1); } 50% { transform: scaleY(0.286); } }
  @keyframes np-eq4 { 0%, 100% { transform: scaleY(0.429); } 50% { transform: scaleY(0.857); } }

  /* Reduced motion */
  @media (prefers-reduced-motion: reduce) {
    .np-eq i { animation: none; }
    .np-ring-spin { animation: none; }
    .np-spin-dot { animation: none; }
    .np-spin-sm { animation: none; }
    .np-cover-spinner { animation: none; }
  }

  .np-cover-spinner {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--border, rgba(255,255,255,0.1));
    border-top-color: var(--accent, #00E4E4);
    animation: np-rot 700ms linear infinite;
  }

  @keyframes np-rot { to { transform: rotate(360deg); } }

  /* Meta (title + breadcrumb) */
  .np-meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .np-title {
    font-size: 12.5px;
    color: var(--fg-1, #e0e0e0);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .np-breadcrumb {
    font-size: 11px;
    color: var(--fg-3, rgba(255,255,255,0.45));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .np-loading-text {
    color: var(--fg-4, rgba(255,255,255,0.3));
    font-style: italic;
  }

  /* Go-to-note button */
  .np-goto-btn {
    width: 26px;
    height: 26px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: var(--fg-3, rgba(255,255,255,0.45));
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    transition: background var(--dur-fast, 0.15s) var(--ease, ease),
                color var(--dur-fast, 0.15s) var(--ease, ease);
  }

  .np-goto-btn:hover:not(:disabled) {
    background: var(--overlay-hover, rgba(255,255,255,0.06));
    color: var(--fg-1, #e0e0e0);
  }

  .np-goto-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* Transport row */
  .np-transport {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
  }

  /* Play/Pause button */
  .np-play-btn {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: none;
    background: var(--surface-2, #1e1e22);
    color: var(--fg-1, #e0e0e0);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    transition: background var(--dur-fast, 0.15s) var(--ease, ease);
  }

  .np-play-btn:hover:not(:disabled) {
    background: var(--surface-3, #2a2a2f);
  }

  .np-play-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Spinning indicator for loading state in transport */
  .np-spin-sm {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 1.5px solid var(--border, rgba(255,255,255,0.1));
    border-top-color: var(--accent, #00E4E4);
    animation: np-rot 700ms linear infinite;
  }

  /* Scrubber */
  .np-scrubber {
    flex: 1;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3, #2a2a2f);
    position: relative;
    cursor: pointer;
    overflow: hidden;
  }

  .np-scrubber:focus {
    outline: 1px solid var(--accent, #00E4E4);
    outline-offset: 2px;
  }

  .np-scrubber-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--accent, #00E4E4);
    border-radius: 2px;
    transition: width 0.1s linear;
  }

  /* Time display */
  .np-time {
    font-family: var(--font-mono, monospace);
    font-size: 10.5px;
    color: var(--fg-3, rgba(255,255,255,0.45));
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .np-time-sep {
    color: var(--fg-4, rgba(255,255,255,0.3));
  }

  /* ── Failed state (expanded) ────────────────────────────────────────────── */
  .np-block--failed {
    /* nothing extra — styling inside children */
  }

  .np-failed-card {
    flex-direction: row;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: rgba(239, 68, 68, 0.05);
    border-top: 1px solid rgba(239, 68, 68, 0.2);
    width: 220px;
    box-sizing: border-box;
  }

  .np-failed-icon {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    background: rgba(239, 68, 68, 0.12);
    color: var(--status-danger, #ef4444);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .np-failed-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .np-failed-title {
    font-size: 12px;
    color: var(--fg-1, #e0e0e0);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .np-failed-sub {
    font-size: 11px;
    color: var(--fg-3, rgba(255,255,255,0.45));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .np-retry-btn {
    height: 24px;
    padding: 0 8px;
    border-radius: 6px;
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    background: var(--surface-2, #1e1e22);
    color: var(--fg-1, #e0e0e0);
    font-size: 11.5px;
    font-weight: 500;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    transition: background var(--dur-fast, 0.15s) var(--ease, ease);
  }

  .np-retry-btn:hover {
    background: var(--surface-3, #2a2a2f);
  }

  /* ── Error toast (#1876-fix) ──────────────────────────────────────────────────── */
  .error-toast {
    position: fixed;
    bottom: 24px;
    right: 24px;
    background: rgba(239,68,68,0.12);
    border: 1px solid rgba(239,68,68,0.35);
    color: #f87171;
    padding: 10px 14px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 9999;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 380px;
    animation: errorToastIn 0.2s ease;
  }
  .error-toast-msg {
    flex: 1;
    line-height: 1.4;
  }
  .error-toast-dismiss {
    background: none;
    border: none;
    color: #f87171;
    cursor: pointer;
    font-size: 14px;
    padding: 0;
    opacity: 0.7;
    flex-shrink: 0;
  }
  .error-toast-dismiss:hover { opacity: 1; }
  @keyframes errorToastIn {
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
  }
</style>
