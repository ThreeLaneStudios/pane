<script lang="ts">
  /**
   * AttachmentNodeView.svelte — TipTap NodeView host for attachment blocks.
   *
   * Renders AttachmentCard + AttachmentToolbar for all attachment types.
   * Wires hover/selection state to the toolbar.
   *
   * PR2 scope: collapsed card only. Modes other than the initial collapsed card
   * are placeholders here; PR4–7 implement full expanded modes.
   *
   * PR3 adds:
   *  - Receives modeChange from toolbar → calls updateAttributes({ renderMode })
   *  - Passes attachmentType to toolbar so the picker can show the right mode list
   *  - Open externally wired from toolbar
   *
   * Click on the card body calls openAttachmentExternally(attachmentId)
   * from the api wrapper PR1 added.
   *
   * §401 PR3 — scope-update.md §PR3.
   */

  import AttachmentCard from './AttachmentCard.svelte';
  import AttachmentToolbar from './AttachmentToolbar.svelte';
  import AttachmentImageInline from './AttachmentImageInline.svelte';
  import AttachmentVideoPlayer from './AttachmentVideoPlayer.svelte';
  import AttachmentPdfReader from './AttachmentPdfReader.svelte';
  import AttachmentHTMLPreview from './AttachmentHTMLPreview.svelte';
  import AttachmentVideoCard from './AttachmentVideoCard.svelte';
  import { openAttachmentExternally, resolveAttachmentSrc, fetchLinkPreview, getCachedLinkPreview } from '$lib/api';
  import { resolveRenderMode } from '$lib/extensions/Attachment';
  import { resolveEmbed } from '$lib/resolvers/resolveEmbed';
  import { loadTrack, playerStatus } from '$lib/stores/player';
  import { activeNote } from '$lib/stores/app';
  import { get } from 'svelte/store';
  import type { Writable } from 'svelte/store';

  // ─── TipTap NodeView props ────────────────────────────────────────────
  // These are bound by the NodeView renderer in Attachment.ts.
  interface NodeAttrs {
    attachmentId?: string | null;
    attachmentType?: string | null;
    filename?: string | null;
    src?: string | null;
    alt?: string | null;
    duration?: number | null;
    pageCount?: number | null;
    renderMode?: string | null;
    /** Explicit width in pixels set by resize (images). null = natural size. */
    width?: number | null;
    /** Explicit height in pixels set by resize (images). null = natural size. */
    height?: number | null;
    /** File size in bytes. null for pre-#1867 markdown that lacked data-size. */
    size?: number | null;
    /** Upload status passed through as a data attribute during upload flow */
    status?: 'uploading' | 'failed' | 'ready' | null;
    /** Error detail for failed state */
    errorDetail?: string | null;
  }

  interface Props {
    /** TipTap node attributes */
    attrs: NodeAttrs;
    /**
     * Whether this node is selected in the editor.
     *
     * Accepts either a plain `boolean` (for direct component usage in tests)
     * or a `Writable<boolean>` store (from the NodeView factory in Attachment.ts).
     * Using a store avoids unmount+remount on every selectNode/deselectNode call,
     * fixing the two-click picker bug (#1843/#401).  Strategy A from Reed audit.
     */
    selected?: boolean | Writable<boolean>;
    /** Called when the user requests a retry (failed state) */
    onRetry?: () => void;
    /**
     * Called when the user changes the render mode via the mode picker.
     * In the live NodeView, this is wired to updateAttributes({ renderMode }).
     * Exposed as a prop so NodeView.test.ts can spy on it without a live TipTap instance.
     *
     * options.skipDirty (fix #2950): pure passthrough to Attachment.ts's updateAttrs.
     * AttachmentImageInline's auto-persist-on-load path sets this to suppress
     * Editor.svelte's dirty-marking for non-user-initiated attribute writes; the
     * mode-picker path below (handleModeChange) never passes it, so real user
     * changes still mark the editor dirty as before.
     */
    onUpdateAttributes?: (attrs: Record<string, unknown>, options?: { skipDirty?: boolean }) => void;
  }

  let { attrs, selected: selectedProp = false, onRetry, onUpdateAttributes }: Props = $props();

  // ─── Resolve selected: store or plain boolean ─────────────────────────────
  // The live NodeView factory passes a Writable<boolean> store so selectNode /
  // deselectNode can push state without unmounting.  Direct renders (tests,
  // Storybook) pass a plain boolean.  Both are accepted here.

  function isStore(v: unknown): v is Writable<boolean> {
    return v !== null && typeof v === 'object' && typeof (v as any).subscribe === 'function';
  }

  // selectedResolved is the boolean selection state that the rest of the
  // component uses.  It's updated by $effect below (store subscription OR
  // plain boolean re-assignment when selectedProp changes).
  let selectedResolved = $state<boolean>(false);

  $effect(() => {
    const s = selectedProp;
    if (isStore(s)) {
      // Subscribe to the store.  The subscribe callback fires immediately with
      // the current value (Svelte store contract), so selectedResolved is
      // initialised correctly.  The return value (unsubscribe) is called by
      // $effect on cleanup (component destroy or prop change).
      return s.subscribe((v: boolean) => {
        selectedResolved = v;
      });
    } else {
      selectedResolved = s as boolean;
    }
  });

  // ─── Hover state ─────────────────────────────────────────────────────────
  let isHovered = $state(false);

  // Toolbar visible when hovered OR selected (CSS also handles hover via :hover).
  // We pass `selected` to AttachmentToolbar to reveal via .is-open.
  let toolbarVisible = $derived(isHovered || selectedResolved);

  // ─── Mode change handler ──────────────────────────────────────────────────
  /**
   * Persists the new render mode to the ProseMirror node attribute.
   * Scope (PR3): attribute persistence ONLY. Rendering changes are PR4–7.
   */
  async function handleModeChange(newMode: string) {
    if (onUpdateAttributes) {
      onUpdateAttributes({ renderMode: newMode });
    } else {
      console.warn(
        '[AttachmentNodeView] onUpdateAttributes is undefined — mode change swallowed. Factory wire broken?',
        { newMode, attachmentType: attrs.attachmentType, attachmentId: attrs.attachmentId }
      );
    }

    // PR5b — wire audio Player mode to NavPlayer.
    // Guard: only audio attachments trigger the player. Even though the mode
    // picker already enforces valid render modes per VALID_RENDER_MODES, this
    // guard is defensive against future regressions.
    if (newMode === 'player' && attrs.attachmentType === 'audio') {
      const note = get(activeNote);
      const noteId = note?.meta?.id ?? null;
      const notebook = note?.meta?.notebook ?? '';
      const title = note?.meta?.title ?? 'Untitled';
      const breadcrumb = notebook ? `${notebook} › ${title}` : title;

      // Defensive: show buffering UI immediately (before the IPC round-trip resolves).
      playerStatus.set('loading');
      const resolvedSrc = await resolveAttachmentSrc(attrs.attachmentId!);
      if (!resolvedSrc) {
        playerStatus.set('failed');
        return;
      }
      loadTrack({
        attachmentId: attrs.attachmentId!,
        noteId,
        title: attrs.filename ?? 'Audio',
        breadcrumb,
        src: resolvedSrc,
      });
    }
  }

  // ─── og: enrichment state (#1780) ────────────────────────────────────────────
  // Only active in the link preview branch (resolveEmbed === null).
  let ogTitle = $state<string | null>(null);
  let ogDescription = $state<string | null>(null);
  let ogImage = $state<string | null>(null);

  // On mount for link preview cards: check cache, then trigger background fetch.
  $effect(() => {
    const src = attrs.src;
    if (attrs.attachmentType !== 'link' || attrs.renderMode !== 'preview' || !src) return;
    // Only fire og fetch for non-video links (resolveEmbed === null branch).
    if (resolveEmbed(src) !== null) return;

    // 1. Check cache synchronously (returns immediately if present).
    getCachedLinkPreview(src).then((cached) => {
      if (cached) {
        ogTitle = cached.og_title ?? null;
        ogDescription = cached.og_description ?? null;
        ogImage = cached.og_image ?? null;
      } else {
        // 2. Cache miss — trigger background fetch. Card shows domain stub while fetching.
        fetchLinkPreview(src).then((data) => {
          if (data) {
            ogTitle = data.og_title ?? null;
            ogDescription = data.og_description ?? null;
            ogImage = data.og_image ?? null;
          }
        }).catch((err) => {
          console.warn('[AttachmentNodeView] og fetch failed:', err);
        });
      }
    }).catch((err) => {
      console.warn('[AttachmentNodeView] og cache check failed:', err);
    });
  });

  // ─── Card click handler ───────────────────────────────────────────────────
  async function handleCardClick(e: MouseEvent) {
    const id = attrs.attachmentId;
    if (!id) return;
    try {
      await openAttachmentExternally(id);
    } catch (err) {
      console.error('[AttachmentNodeView] openAttachmentExternally failed:', err);
    }
  }

  // ─── Toolbar actions ──────────────────────────────────────────────────────
  async function handleOpenExternally() {
    const id = attrs.attachmentId;
    if (!id) return;
    try {
      await openAttachmentExternally(id);
    } catch (err) {
      console.error('[AttachmentNodeView] openAttachmentExternally failed:', err);
    }
  }
</script>

<!--
  The outer wrapper carries data-* attrs so the serializer (Editor.svelte) can
  still find them when serializing back to markdown. The .attachment-wrapper
  class is expected by the serializer's querySelector checks.
-->
<div
  class="attachment-wrapper at-nv-host"
  class:is-selected={selectedResolved}
  data-attachment=""
  data-attachment-id={attrs.attachmentId ?? undefined}
  data-attachment-type={attrs.attachmentType ?? undefined}
  data-filename={attrs.filename ?? undefined}
  data-render-mode={attrs.renderMode ?? undefined}
  data-width={attrs.width != null ? String(attrs.width) : undefined}
  data-height={attrs.height != null ? String(attrs.height) : undefined}
  role="presentation"
  onmouseenter={() => { isHovered = true; }}
  onmouseleave={() => { isHovered = false; }}
>
  <!-- Floating toolbar: positioned absolute, won't push content -->
  <AttachmentToolbar
    renderMode={attrs.renderMode}
    attachmentType={attrs.attachmentType}
    selected={toolbarVisible}
    onModeChange={handleModeChange}
    onOpenExternally={handleOpenExternally}
  />

  <!-- Conditional render: full-inline image / video player / card fallback -->
  {#if attrs.attachmentType === 'image' && resolveRenderMode(attrs.attachmentType, attrs.renderMode) === 'full-inline'}
    <AttachmentImageInline
      attachmentId={attrs.attachmentId ?? ''}
      filename={attrs.filename ?? ''}
      width={attrs.width ?? null}
      height={attrs.height ?? null}
      selected={selectedResolved}
      onUpdateAttributes={onUpdateAttributes}
    />
  {:else if attrs.attachmentType === 'video' && attrs.renderMode === 'player'}
    <AttachmentVideoPlayer
      attachmentId={attrs.attachmentId ?? ''}
      filename={attrs.filename ?? ''}
      duration={attrs.duration ?? null}
    />
  {:else if attrs.attachmentType === 'pdf' && attrs.renderMode === 'reader'}
    <AttachmentPdfReader
      attachmentId={attrs.attachmentId ?? ''}
      filename={attrs.filename ?? ''}
    />
  {:else if attrs.attachmentType === 'html' && attrs.renderMode === 'preview'}
    <AttachmentHTMLPreview
      attachmentId={attrs.attachmentId ?? ''}
      filename={attrs.filename ?? ''}
    />
  {:else if attrs.attachmentType === 'link' && attrs.renderMode === 'plain'}
    <!-- Plain link: bare clickable URL, no fetch, no loader (LOCKED-DESIGN-SPEC plain mode) -->
    <a
      href={attrs.src ?? ''}
      target="_blank"
      rel="noopener noreferrer"
      class="at-link-plain"
      data-attachment-type="link"
      data-render-mode="plain"
    >{attrs.src ?? ''}</a>
  {:else if attrs.attachmentType === 'link' && attrs.renderMode === 'preview'}
    <!-- Preview link: video card for allowlisted providers, domain stub for all others (#1902). -->
    {#if attrs.src && resolveEmbed(attrs.src) !== null}
      {@const embed = resolveEmbed(attrs.src)!}
      <AttachmentVideoCard
        provider={embed.provider}
        videoId={embed.videoId}
        filename={attrs.filename}
        src={attrs.src}
        selected={selectedResolved}
      />
    {:else}
      <!-- Non-allowlisted link: og-enriched card (#1780 ADR-0049).
           ogTitle/ogDescription/ogImage populated by $effect above (cache or background fetch).
           Falls back to extractDomain(src) when og fields are null (domain stub path). -->
      <AttachmentCard
        attachment={{
          attachmentId: attrs.attachmentId,
          attachmentType: 'link',
          filename: attrs.filename,
          src: attrs.src,
          alt: attrs.alt,
          duration: attrs.duration,
          pageCount: attrs.pageCount,
          width: attrs.width,
          height: attrs.height,
          size: attrs.size,
          status: attrs.status,
          errorDetail: attrs.errorDetail,
          ogTitle,
          ogDescription,
          ogImage,
        }}
        selected={selectedResolved}
        {onRetry}
        onClick={handleCardClick}
      />
    {/if}
  {:else}
    <!-- Card fallback: all other types + non-expanded modes for image/video -->
    <AttachmentCard
      attachment={{
        attachmentId: attrs.attachmentId,
        attachmentType: attrs.attachmentType,
        filename: attrs.filename,
        src: attrs.src,
        alt: attrs.alt,
        duration: attrs.duration,
        pageCount: attrs.pageCount,
        width: attrs.width,
        height: attrs.height,
        size: attrs.size,
        status: attrs.status,
        errorDetail: attrs.errorDetail,
      }}
      selected={selectedResolved}
      {onRetry}
      onClick={handleCardClick}
    />
  {/if}
</div>

<style>
  /*
   * Host wrapper: positions the floating toolbar absolutely above the card.
   * No height constraint here — the card enforces 64px. Margin adds breathing
   * room in the note flow so the revealed toolbar (top:-16px) doesn't clip.
   */
  .at-nv-host {
    position: relative;
    display: block;
    margin: 20px 0 8px; /* top: 20px gives the floating toolbar room to show */
  }

  /*
   * Reveal toolbar on host hover (complements .is-open on selected).
   * The .at-toolbar inside AttachmentToolbar becomes visible via CSS
   * when the parent host is hovered.
   */
  .at-nv-host:hover :global(.at-toolbar),
  .at-nv-host.is-selected :global(.at-toolbar) {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }

  /*
   * Plain link mode: inline-block URL anchor, no decoration beyond underline.
   * Inherits editor prose font. Opens externally via Tauri shell.openExternal.
   */
  .at-link-plain {
    display: inline-block;
    color: var(--color-accent, #4a90d9);
    text-decoration: underline;
    word-break: break-all;
    font-size: inherit;
    font-family: inherit;
  }
</style>
