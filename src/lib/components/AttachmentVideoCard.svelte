<script lang="ts">
  /**
   * AttachmentVideoCard.svelte — Rich video card for allowlisted providers (#1902).
   *
   * Replaces the thin "File / domain" stub for YouTube and Vimeo link attachments
   * in `link + preview` render mode. Shows a provider thumbnail (YouTube CDN),
   * title, and an "Open in [Provider]" button that launches the canonical watch
   * URL in the system browser via tauri-plugin-shell.
   *
   * Vimeo thumbnail: deferred (oEmbed API call required). v1 renders a play-icon
   * placeholder instead.
   *
   * Design tokens: same as AttachmentCard (--at-card-h, --surface-1/2,
   * --fg-1/2/3, --border, --border-strong, --accent, --dur-fast, --ease).
   *
   * §401 #1902 — docs/spec/video-card.md.
   */

  import { open } from '@tauri-apps/plugin-shell';
  import { providerThumbnail } from '$lib/resolvers/resolveEmbed';

  interface Props {
    provider: 'youtube' | 'vimeo';
    videoId: string;
    /** Display title — falls back to watch URL when null/empty. */
    filename?: string | null;
    /** Original user-supplied URL (used only as last-resort title fallback). */
    src?: string | null;
    selected?: boolean;
  }

  let { provider, videoId, filename, src, selected = false }: Props = $props();

  // ─── Derived ─────────────────────────────────────────────────────────────

  let watchUrl = $derived(
    provider === 'youtube'
      ? `https://www.youtube.com/watch?v=${videoId}`
      : `https://vimeo.com/${videoId}`
  );

  let providerLabel = $derived(provider === 'youtube' ? 'YouTube' : 'Vimeo');

  let buttonLabel = $derived(`Open in ${providerLabel}`);

  /** Title shown on the card — prefer filename attr, fall back to watch URL. */
  let title = $derived(
    (filename && filename.trim()) ? filename.trim() : (src ?? watchUrl)
  );

  /** YouTube CDN thumbnail URL; null for Vimeo (show placeholder). */
  let thumbnailUrl = $derived(providerThumbnail(provider, videoId));

  // ─── Handlers ────────────────────────────────────────────────────────────

  async function handleOpen(e: MouseEvent | KeyboardEvent) {
    e.stopPropagation();
    await open(watchUrl);
  }
</script>

<div class="at-block" class:is-selected={selected}>
  <div
    class="at-video-card"
    data-provider={provider}
    role="group"
    aria-label="{providerLabel} video: {title}"
  >
    <!-- Thumbnail column -->
    <span class="at-thumb" aria-hidden="true">
      {#if thumbnailUrl}
        <img
          src={thumbnailUrl}
          alt=""
          class="at-thumb-img"
          loading="lazy"
          draggable="false"
        />
        <!-- Play overlay -->
        <span class="at-play-overlay" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <polygon points="5,3 19,12 5,21" />
          </svg>
        </span>
      {:else}
        <!-- Vimeo placeholder (thumbnail deferred) -->
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <polygon points="5,3 19,12 5,21" />
        </svg>
      {/if}
    </span>

    <!-- Title + provider meta -->
    <span class="at-main">
      <span class="at-name">{title}</span>
      <span class="at-meta">{providerLabel}</span>
    </span>

    <!-- "Open in [Provider]" button -->
    <span class="at-trail">
      <button
        class="at-open-btn"
        type="button"
        aria-label={buttonLabel}
        onclick={handleOpen}
        onkeydown={(e) => e.key === 'Enter' && handleOpen(e)}
      >
        <!-- External link icon -->
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M14 4h6v6"/>
          <path d="M20 4 10 14"/>
          <path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>
        </svg>
        {buttonLabel}
      </button>
    </span>
  </div>
</div>

<style>
  /* ── Block wrapper: selection ring host ─────────────────────────────── */
  .at-block {
    position: relative;
  }

  /* ── Video card row — same 64px height as AttachmentCard ────────────── */
  .at-video-card {
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--at-card-h, 64px);
    box-sizing: border-box;
    padding: 0 14px;
    background: var(--surface-1);
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    border-radius: 10px;
    user-select: none;
    transition:
      border-color var(--dur-fast, 120ms) var(--ease, ease),
      background var(--dur-fast, 120ms) var(--ease, ease);
  }
  .at-block:hover .at-video-card {
    border-color: var(--border-strong, rgba(255,255,255,0.2));
  }
  .at-block.is-selected .at-video-card {
    box-shadow: 0 0 0 1px var(--accent, #00E4E4);
    border-color: var(--accent, #00E4E4);
  }

  /* ── Thumbnail tile (38×38, same proportions as .at-ico) ────────────── */
  .at-thumb {
    position: relative;
    width: 38px;
    height: 38px;
    flex-shrink: 0;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-2, rgba(255,255,255,0.06));
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-2, rgba(255,255,255,0.6));
  }
  .at-thumb-img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    border-radius: 6px;
  }
  /* Play icon overlay on top of thumbnail */
  .at-play-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.32);
    color: #fff;
    border-radius: 6px;
  }

  /* ── Title + provider text column ───────────────────────────────────── */
  .at-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  .at-name {
    font-size: 15px;
    color: var(--fg-1, #e0e0e0);
    font-weight: 500;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .at-meta {
    font-size: 12.5px;
    color: var(--fg-3, rgba(255,255,255,0.4));
    margin-top: 2px;
  }

  /* ── "Open in [Provider]" button ────────────────────────────────────── */
  .at-trail {
    flex-shrink: 0;
    display: flex;
    align-items: center;
  }
  .at-open-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border, rgba(255,255,255,0.12));
    background: var(--surface-2, rgba(255,255,255,0.06));
    color: var(--fg-2, rgba(255,255,255,0.6));
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background var(--dur-fast, 120ms) var(--ease, ease),
      border-color var(--dur-fast, 120ms) var(--ease, ease),
      color var(--dur-fast, 120ms) var(--ease, ease);
  }
  .at-open-btn:hover {
    background: var(--surface-2-hover, rgba(255,255,255,0.1));
    border-color: var(--border-strong, rgba(255,255,255,0.2));
    color: var(--fg-1, #e0e0e0);
  }
  .at-open-btn:focus-visible {
    outline: 2px solid var(--accent, #00E4E4);
    outline-offset: 2px;
  }
  .at-open-btn:active {
    opacity: 0.75;
  }
</style>
