<script lang="ts">
  /**
   * AttachmentVideoPlayer.svelte — Inline video player for PR4 (#401).
   *
   * Features:
   *  - 16:9 aspect-ratio container (CSS, not fixed px height).
   *  - Native <video controls> as the controls bar (first-pass; design-system
   *    styled via CSS custom properties where the WebView supports it).
   *  - Autoplay OFF. Nothing plays until the user presses play.
   *  - Poster: uses browser's natural first-frame preview via preload="metadata".
   *    No poster extraction logic (no canvas, no ffmpeg, no thumbnail service).
   *  - Src resolved via resolveAttachmentSrc(attachmentId) → asset:// URL. Never base64.
   *
   * Scope: video player inline only. PR5 handles audio. Card fallback remains in
   * AttachmentNodeView for video in card mode.
   */

  import { onMount } from 'svelte';
  import { resolveAttachmentSrc } from '$lib/api';

  interface Props {
    attachmentId: string;
    filename: string;
    duration?: number | null;
  }

  let { attachmentId, filename, duration = null }: Props = $props();

  let resolvedSrc = $state('');

  onMount(async () => {
    resolvedSrc = await resolveAttachmentSrc(attachmentId);
  });

  function formatDuration(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    return `${m}:${String(s).padStart(2, '0')}`;
  }
</script>

<div class="at-video-player">
  <div class="at-video-ratio">
    <!--
      Always render <video> (not conditional on resolvedSrc) so it's always in
      the DOM for tests and for accessibility. src='' before resolution; non-empty
      after onMount. autoplay={false} → Svelte removes the attribute.
    -->
    <video
      class="at-video"
      src={resolvedSrc}
      controls
      preload="metadata"
      autoplay={false}
      title={filename}
    >
      <track kind="captions" />
      Your browser does not support video playback.
    </video>
  </div>
  {#if duration !== null && duration !== undefined}
    <div class="at-video-meta">
      <span class="at-video-duration">{formatDuration(duration)}</span>
    </div>
  {/if}
</div>

<style>
  /* Outer block */
  .at-video-player {
    display: block;
    width: 100%;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-secondary, #111);
  }

  /* 16:9 aspect-ratio container */
  .at-video-ratio {
    position: relative;
    width: 100%;
    aspect-ratio: 16 / 9;
    background: #000;
  }

  .at-video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    background: #000;
    /* Style the native controls to match design tokens as much as possible.
       Full custom controls are a PR5+ enhancement. */
    accent-color: var(--accent, #00E4E4);
  }

  /* Duration meta line below the video */
  .at-video-meta {
    display: flex;
    align-items: center;
    padding: 4px 10px;
    gap: 6px;
  }

  .at-video-duration {
    font-size: 11px;
    color: var(--text-tertiary, rgba(255,255,255,0.45));
    font-variant-numeric: tabular-nums;
  }
</style>
