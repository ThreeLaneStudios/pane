<script lang="ts">
  /**
   * AttachmentCard.svelte — Fixed 64px collapsed card for all 7 attachment types.
   *
   * States:
   *  - Default: icon tile + name + meta + trailing chevron/external-link glyph
   *  - Loading skeleton: shimmer bars over icon-tile/name/meta layout
   *  - Upload failed: red accent on icon tile, error message in meta, retry hint
   *
   * Design tokens consumed from PR1 (app.css):
   *   --at-card-h, --accent, --surface-1, --surface-2, --text-1, --text-2,
   *   --border-1, --border, --fg-1, --fg-2, --fg-3, --fg-on-accent, --status-danger
   *
   * §401 PR2 — LOCKED-DESIGN-SPEC.md + attachments.css reference.
   */

  interface AttachmentMeta {
    attachmentId?: string | null;
    attachmentType?: string | null;
    filename?: string | null;
    src?: string | null;
    alt?: string | null;
    /** Duration in seconds (audio/video) */
    duration?: number | null;
    /** Page count (PDF) */
    pageCount?: number | null;
    /** Image width in pixels */
    width?: number | null;
    /** Image height in pixels */
    height?: number | null;
    /** File size in bytes */
    size?: number | null;
    /** Upload status */
    status?: 'uploading' | 'failed' | 'ready' | null;
    /** Error detail for failed state */
    errorDetail?: string | null;
    /** og: enrichment fields (#1780, ADR-0049). Null → fall back to extractDomain. */
    ogTitle?: string | null;
    ogDescription?: string | null;
    ogImage?: string | null;
  }

  interface Props {
    attachment: AttachmentMeta;
    selected?: boolean;
    onRetry?: () => void;
    onClick?: (e: MouseEvent) => void;
  }

  let { attachment, selected = false, onRetry, onClick }: Props = $props();

  // ─── Helpers ─────────────────────────────────────────────────────────────

  function formatBytes(bytes: number | null | undefined): string {
    if (bytes == null || bytes <= 0) return '';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  function formatDuration(secs: number | null | undefined): string {
    if (secs == null || secs < 0) return '';
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    return `${m}:${String(s).padStart(2, '0')}`;
  }

  /**
   * Returns the type-prefix segment for the meta line per #1774 locked design.
   * PDF and HTML always use their fixed label.
   * Audio, video, and file use the uppercase extension from filename;
   * falls back to AUDIO / VIDEO / FILE when the extension cannot be determined.
   * Image and link return '' (no prefix — self-identifying or domain-only).
   */
  function typePrefix(attachmentType: string | null | undefined, filename: string | null | undefined): string {
    const t = attachmentType ?? 'file';
    if (t === 'pdf') return 'PDF';
    if (t === 'html') return 'HTML';
    if (t === 'image' || t === 'link') return '';
    // audio, video, file — derive from filename extension
    if (filename) {
      const dot = filename.lastIndexOf('.');
      if (dot >= 0 && dot < filename.length - 1) {
        return filename.slice(dot + 1).toUpperCase();
      }
    }
    if (t === 'audio') return 'AUDIO';
    if (t === 'video') return 'VIDEO';
    return 'FILE';
  }

  function extractDomain(filename: string | null | undefined): string {
    if (!filename) return '';
    try {
      const url = filename.startsWith('http') ? filename : `https://${filename}`;
      return new URL(url).hostname.replace(/^www\./, '');
    } catch {
      return filename;
    }
  }

  // ─── Derived state ───────────────────────────────────────────────────────

  let type = $derived(attachment.attachmentType ?? 'file');
  // og:title takes priority over filename for link cards (#1780).
  let name = $derived(
    type === 'link' && attachment.ogTitle
      ? attachment.ogTitle
      : (attachment.filename ?? attachment.alt ?? 'File')
  );
  let isLoading = $derived(
    attachment.attachmentType !== 'link' &&
    (attachment.status === 'uploading' || (!attachment.status && !attachment.filename && !attachment.attachmentId))
  );
  let isFailed = $derived(attachment.status === 'failed');

  let metaLine = $derived((): string => {
    if (isFailed) {
      return attachment.errorDetail ?? 'Upload failed';
    }
    if (isLoading) return '';

    const bytes = formatBytes(attachment.size);
    const prefix = typePrefix(attachment.attachmentType, attachment.filename);

    switch (type) {
      case 'image': {
        // No type prefix for images — visually self-identifying per #1774 design.
        const dims = attachment.width && attachment.height
          ? `${attachment.width}×${attachment.height}`
          : '';
        return [dims, bytes].filter(Boolean).join(' · ');
      }
      case 'pdf': {
        // #1774: PDF · {bytes} · {N page[s]}
        const n = attachment.pageCount;
        const pages = n ? `${n} ${n === 1 ? 'page' : 'pages'}` : '';
        return [prefix, bytes, pages].filter(Boolean).join(' · ');
      }
      case 'audio': {
        // #1774: {EXT} · {bytes} — no duration segment for audio
        return [prefix, bytes].filter(Boolean).join(' · ');
      }
      case 'video': {
        // #1774: {EXT} · {duration} · {bytes}
        const dur = formatDuration(attachment.duration);
        return [prefix, dur, bytes].filter(Boolean).join(' · ');
      }
      case 'html': {
        return [prefix, bytes].filter(Boolean).join(' · ');
      }
      case 'link': {
        // og:description takes priority; fall back to domain stub (#1780).
        if (attachment.ogDescription) return attachment.ogDescription;
        return extractDomain(attachment.filename ?? attachment.src);
      }
      default:
        // file and unknown types: {EXT} · {bytes} or FILE · {bytes}
        return [prefix, bytes].filter(Boolean).join(' · ');
    }
  });

  // Map attachment type to icon SVG path data.
  // Using Lucide-style 1.6px stroke icons matching attachment-parts.jsx.
  type IconKey = 'image' | 'pdf' | 'audio' | 'video' | 'html' | 'link' | 'file' | 'alert';

  const ICON_PATHS: Record<IconKey, string> = {
    image:  'M3 3h18v18H3V3z M3 3a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2M9 9a1.6 1.6 0 1 0 0-3.2A1.6 1.6 0 0 0 9 9zm12 6l-5-5L5 21',
    pdf:    'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M9 13h1.5a1.5 1.5 0 0 1 0 3H9z M9 16v-3',
    audio:  'M11 5 6 9H2v6h4l5 4z M15.5 8.5a5 5 0 0 1 0 7 M19 5a9 9 0 0 1 0 14',
    video:  'M2 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H2a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z m22 8-6 4 6 4z',
    html:   'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 m-14 4-2 2 2 2 m4-4 2 2-2 2',
    link:   'M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.5 1.5 M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7L12 19',
    file:   'M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z M13 2v7h7',
    alert:  'M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z M12 9v4 M12 17h.01',
  };

  function getIconKey(t: string): IconKey {
    if (t === 'image') return 'image';
    if (t === 'pdf') return 'pdf';
    if (t === 'audio') return 'audio';
    if (t === 'video') return 'video';
    if (t === 'html') return 'html';
    if (t === 'link') return 'link';
    return 'file';
  }

  let iconKey = $derived(isFailed ? 'alert' as IconKey : getIconKey(type));
  let iconPaths = $derived(ICON_PATHS[iconKey].split(' M ').filter(Boolean));
</script>

<!-- at-block wrapper: positions the floating toolbar + selection ring -->
<div
  class="at-block"
  class:is-selected={selected}
  role="presentation"
>
  <!-- ── Loading skeleton ─────────────────────────────────────────────── -->
  {#if isLoading}
    <div class="at-card is-loading" aria-label="Loading attachment" aria-busy="true">
      <!-- skeleton icon tile -->
      <span class="at-ico">
        <span class="at-skel" style="width:22px;height:22px;border-radius:4px;"></span>
      </span>
      <!-- skeleton text lines -->
      <span class="at-main">
        <span class="at-skel" style="width:55%;height:12px;border-radius:4px;margin-bottom:5px;display:block;"></span>
        <span class="at-skel" style="width:35%;height:9px;border-radius:4px;display:block;"></span>
      </span>
      <span class="at-trail">
        <span class="at-spin" aria-hidden="true"></span>
      </span>
    </div>

  <!-- ── Upload-failed state ──────────────────────────────────────────── -->
  {:else if isFailed}
    <div
      class="at-card is-failed"
      onclick={onClick}
      role="button"
      tabindex="0"
      onkeydown={(e) => e.key === 'Enter' && onClick?.(e as unknown as MouseEvent)}
      aria-label={`Failed attachment: ${name}`}
    >
      <span class="at-ico at-ico--failed" aria-hidden="true">
        <!-- Alert triangle icon -->
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
          <path d="M12 9v4"/>
          <path d="M12 17h.01"/>
        </svg>
      </span>
      <span class="at-main">
        <span class="at-fail-title">{name}</span>
        <span class="at-fail-sub">{attachment.errorDetail ?? "Couldn't upload this file"}</span>
      </span>
      {#if onRetry}
        <span class="at-trail">
          <button class="at-retry" onclick={(e) => { e.stopPropagation(); onRetry?.(); }} type="button" aria-label="Retry upload">
            <!-- Retry icon -->
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M3 12a9 9 0 1 0 3-6.7L3 8"/>
              <path d="M3 3v5h5"/>
            </svg>
            Retry
          </button>
        </span>
      {/if}
    </div>

  <!-- ── Default (loaded) card ────────────────────────────────────────── -->
  {:else}
    <div
      class="at-card"
      onclick={onClick}
      role="button"
      tabindex="0"
      onkeydown={(e) => e.key === 'Enter' && onClick?.(e as unknown as MouseEvent)}
      aria-label={name}
      data-attachment-type={type}
    >
      <!-- Remote og:image URLs are intentionally not loaded in the WebView.
           Text metadata remains available; thumbnails require a future guarded
           backend proxy/cache. Allowlisted video cards own their canonical CDN URLs. -->
      <span class="at-ico" aria-hidden="true">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          {#each iconPaths as d}
            <path {d}/>
          {/each}
        </svg>
      </span>

      <!-- Name + meta -->
      <span class="at-main">
        <span class="at-name">{name}</span>
        {#if metaLine()}
          <span class="at-meta">{metaLine()}</span>
        {/if}
      </span>

      <!-- Trailing affordance: chevron-right for most types, external-link for link -->
      <span class="at-trail" aria-hidden="true">
        {#if type === 'link'}
          <!-- External link glyph -->
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M14 4h6v6"/>
            <path d="M20 4 10 14"/>
            <path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>
          </svg>
        {:else}
          <!-- Chevron-right -->
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 6 6 6-6 6"/>
          </svg>
        {/if}
      </span>
    </div>
  {/if}
</div>

<style>
  /* ── Block wrapper: enables toolbar positioning + selection ring ────── */
  .at-block {
    position: relative;
  }

  /* ── Canonical collapsed card — fixed 64px height ───────────────────── */
  .at-card {
    display: flex;
    align-items: center;
    gap: 14px;
    height: var(--at-card-h, 64px);
    box-sizing: border-box;
    padding: 0 14px;
    background: var(--surface-1);
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    border-radius: 10px;
    cursor: pointer;
    user-select: none;
    transition:
      border-color var(--dur-fast, 120ms) var(--ease, ease),
      background var(--dur-fast, 120ms) var(--ease, ease);
  }
  .at-card:focus {
    outline: none;
  }
  .at-block:hover .at-card {
    border-color: var(--border-strong, rgba(255,255,255,0.2));
  }
  .at-block.is-selected .at-card {
    box-shadow: 0 0 0 1px var(--accent, #00E4E4);
    border-color: var(--accent, #00E4E4);
  }

  /* ── Failed card variant ────────────────────────────────────────────── */
  .at-card.is-failed {
    background: rgba(239, 68, 68, 0.05);
    border-color: rgba(239, 68, 68, 0.28);
  }

  /* ── Loading card ───────────────────────────────────────────────────── */
  .at-card.is-loading {
    pointer-events: none;
    cursor: default;
  }

  /* ── Icon tile (38×38 inside 64px row) ─────────────────────────────── */
  .at-ico {
    width: 38px;
    height: 38px;
    flex-shrink: 0;
    border-radius: 8px;
    background: var(--surface-2, rgba(255,255,255,0.06));
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-2, rgba(255,255,255,0.6));
    overflow: hidden;
  }


  .at-ico--failed {
    background: rgba(239, 68, 68, 0.12);
    color: var(--status-danger, #EF4444);
  }

  /* ── Name + meta text column ────────────────────────────────────────── */
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
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* design source: attachments.css:46 — bold inline values (page counts etc.)
     should be one shade brighter than the surrounding meta text */
  .at-meta b {
    color: var(--fg-2, rgba(255,255,255,0.6));
    font-weight: 500;
  }

  /* ── Failed state text ──────────────────────────────────────────────── */
  .at-fail-title {
    color: var(--fg-1, #e0e0e0);
    font-weight: 500;
    font-size: 14.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .at-fail-sub {
    font-size: 12.5px;
    color: var(--fg-3, rgba(255,255,255,0.4));
    margin-top: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* ── Trailing affordance ────────────────────────────────────────────── */
  .at-trail {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--fg-3, rgba(255,255,255,0.4));
  }

  /* ── Retry button ───────────────────────────────────────────────────── */
  .at-retry {
    height: 30px;
    padding: 0 12px;
    border-radius: 7px;
    background: var(--surface-2, rgba(255,255,255,0.06));
    border: 1px solid var(--border, rgba(255,255,255,0.1));
    color: var(--fg-1, #e0e0e0);
    font-family: var(--font-sans, inherit);
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    transition: background var(--dur-fast, 120ms) var(--ease, ease);
  }
  .at-retry:hover {
    background: var(--surface-3, rgba(255,255,255,0.1));
  }

  /* ── Loading skeleton + shimmer ─────────────────────────────────────── */
  .at-skel {
    background: var(--surface-2, rgba(255,255,255,0.06));
    border-radius: 5px;
    position: relative;
    overflow: hidden;
  }
  .at-skel::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(
      90deg,
      transparent,
      rgba(240, 237, 232, 0.06),
      transparent
    );
    transform: translateX(-100%);
    animation: at-shimmer 1.25s ease-in-out infinite;
  }
  @keyframes at-shimmer {
    to { transform: translateX(100%); }
  }

  /* ── Upload spinner ─────────────────────────────────────────────────── */
  .at-spin {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    border: 2px solid var(--border, rgba(255,255,255,0.1));
    border-top-color: var(--accent, #00E4E4);
    border-radius: 50%;
    animation: at-rot 700ms linear infinite;
  }
  @keyframes at-rot {
    to { transform: rotate(360deg); }
  }
</style>
