<script lang="ts">
  /**
   * AttachmentImageInline.svelte — Full-inline resizable image for PR4 (#401).
   *
   * Features:
   *  - Renders the attachment image at natural size, or at persisted width/height.
   *  - Four corner drag handles for resizing; aspect-ratio locked by default.
   *  - Dimension overlay during drag (hides ~500ms after drag ends).
   *  - Cyan selection ring when `selected` prop is true.
   *  - On drag-end: calls onUpdateAttributes({ width, height }) to persist dims.
   *  - Src resolved via resolveAttachmentSrc(attachmentId) → asset:// URL. Never base64.
   *
   * Scope: image full-inline only. PR5/6/7 handle other types.
   */

  import { onMount, onDestroy, untrack } from 'svelte';
  import { resolveAttachmentSrc } from '$lib/api';

  interface Props {
    attachmentId: string;
    filename: string;
    width?: number | null;
    height?: number | null;
    selected?: boolean;
    // options.skipDirty (fix #2950): threaded through to Attachment.ts's updateAttrs
    // so the caller can tag the ProseMirror transaction with TipTap's native
    // 'preventUpdate' meta, suppressing Editor.svelte's onUpdate (and therefore
    // dirty=true) for non-user-initiated attribute writes. See handleImageLoad().
    onUpdateAttributes?: (attrs: Record<string, unknown>, options?: { skipDirty?: boolean }) => void;
  }

  let {
    attachmentId,
    filename,
    width = null,
    height = null,
    selected = false,
    onUpdateAttributes,
  }: Props = $props();

  // ── Resolved src (asset:// URL) ──────────────────────────────────────────
  let resolvedSrc = $state('');

  // ── Current display dimensions ────────────────────────────────────────────
  // naturalWidth/naturalHeight come from the loaded <img> element.
  let naturalW = $state(0);
  let naturalH = $state(0);

  // Current (possibly drag-in-progress) render dimensions.
  // Seeded from persisted attrs; falls back to natural once image loads.
  let currentW = $state(0);
  let currentH = $state(0);

  // ── Drag state ────────────────────────────────────────────────────────────
  let isDragging = $state(false);
  let showOverlay = $state(false);
  let overlayTimer: ReturnType<typeof setTimeout> | null = null;

  // Drag bookkeeping
  let dragStartX = 0;
  let dragStartY = 0;
  let dragStartW = 0;
  let dragStartH = 0;
  let dragCorner: 'tl' | 'tr' | 'bl' | 'br' = 'br';
  let aspectRatio = 1;

  // Cached .pane-editor element for drag-start column width capture.
  // Set at startDrag; used in applyDrag to clamp currentW to live column width.
  let cachedColumnEl: Element | null = null;

  // ── Sync persisted dims from props ───────────────────────────────────────
  // $effect runs whenever width/height PROPS change (e.g. node update from TipTap).
  // We do NOT read isDragging inside this effect — that was the bug: setting
  // isDragging=false in onDragEnd() re-fired the effect and overwrote the drag
  // result with stale prop values before TipTap's update() arrived.
  // Instead: always sync from props, but guard on whether a drag is in progress
  // by reading isDragging via untrack so it's not a reactive dependency.
  $effect(() => {
    // Track width/height props as reactive deps (effect re-runs only when they change).
    const w = width ?? 0;
    const h = height ?? 0;
    // Read isDragging without making it a dep — if a drag is in progress, don't
    // clobber the drag result with the (not-yet-updated) prop value.
    if (!untrack(() => isDragging)) {
      currentW = w;
      currentH = h;
    }
  });

  // ── Mount: resolve src ────────────────────────────────────────────────────
  onMount(async () => {
    resolvedSrc = await resolveAttachmentSrc(attachmentId);
  });

  // ── Cleanup: remove document-scope drag listeners on unmount ─────────────
  // Guards against orphaned listeners if the component is destroyed mid-drag
  // (e.g. user navigates away while dragging a resize handle). W-4 fix.
  onDestroy(() => {
    document.removeEventListener('mousemove', onDragMove);
    document.removeEventListener('mouseup', onDragEnd);
  });

  // ── Image load: capture natural dimensions ────────────────────────────────
  function handleImageLoad(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    naturalW = img.naturalWidth;
    naturalH = img.naturalHeight;
    aspectRatio = naturalH > 0 ? naturalW / naturalH : 1;

    // If no persisted dims, use natural size.
    if (!currentW || currentW === 0) currentW = naturalW;
    if (!currentH || currentH === 0) currentH = naturalH;

    // Persist natural dims to node attrs only when they are null (first load).
    // Guard: never overwrite user-resized values that are already stored.
    //
    // skipDirty: true (fix #2950) — this persist is triggered by the native <img>
    // `load` event, not by the user. It can fire asynchronously AFTER the editor's
    // isSettingContent flag has already been reset by the initial setContent() call,
    // so without this flag the write would incorrectly mark the editor dirty and
    // trigger a save on a note the user never touched. Real user edits (drag-resize
    // in onDragEnd below) intentionally omit this option so they still mark dirty.
    if (width === null && height === null) {
      if (typeof onUpdateAttributes === 'function') {
        onUpdateAttributes({ width: naturalW, height: naturalH }, { skipDirty: true });
      } else {
        console.warn(
          '[AttachmentImageInline] onUpdateAttributes is undefined — natural dims not persisted. Factory wire broken?',
          { naturalW, naturalH, attachmentId }
        );
      }
    }
  }

  // ── Computed style for the .at-image-inline container ─────────────────────
  // Bug 1 round 3 fix (#1843): container must carry currentW so that:
  //   1. Resize handles (absolute-positioned to container) hug the image edge,
  //      not the column edge.
  //   2. Drag is visibly continuous at ALL widths, not just below ~720px.
  // When currentW=0 (pre-natural-dims), fall back to max-width:100% so the
  // container renders at its natural/intrinsic size unconstrained.
  let containerStyle = $derived(
    currentW > 0
      ? `width:${currentW}px; max-width:100%;`
      : 'max-width:100%;'
  );

  // ── Computed style for the image ─────────────────────────────────────────
  // width:min(px, 100%) — the binding constraint is whichever is
  // smaller: the dragged pixel value OR the container width (100% of parent).
  //
  // Bug fixed (Bug 1, #1843): the previous 
  // pattern only scaled below the editor column width (~720px). Above 720px,
  //  filled the container and  was non-binding —
  // dragging had no visible effect.  directly constrains the
  // visible pixel width at ALL sizes:
  //   • currentW=300 (<720): min(300px, 100%) = 300px; height = 300*(h/w)
  //   • currentW=700 (<720): min(700px, 100%) = 700px; height = 700*(h/w)
  //   • currentW=1500 (>720): min(1500px, 100%) = 720px; height = 720*(h/w)
  //
  // No height-squish: no explicit height in px. aspect-ratio drives height
  // from whatever width the browser resolves — proportional at any size.
  //
  // Drag-resize path: currentW/currentH updated every tick; aspect-ratio
  // follows, so resizing still works continuously at any size.
  // Loading gap branch (naturalW/H known, currentW/H not yet set): keep
  // max-width:100%; aspect-ratio pattern — already correct.
  let imageStyle = $derived((() => {
    if (currentW > 0 && currentH > 0) {
      return `width:min(${currentW}px, 100%); aspect-ratio:${currentW}/${currentH};`;
    }
    if (naturalW > 0 && naturalH > 0) {
      return `max-width:100%;aspect-ratio:${naturalW}/${naturalH};`;
    }
    return 'max-width:100%';
  })());

  // ── Corner drag handlers ──────────────────────────────────────────────────
  function startDrag(e: MouseEvent | TouchEvent, corner: 'tl' | 'tr' | 'bl' | 'br') {
    e.preventDefault();
    e.stopPropagation();

    dragCorner = corner;

    // Cache the enclosing .pane-editor element FIRST so we can compute columnMaxPx
    // before capturing the drag anchor. Previously captured after dragStartW; that
    // caused the anchor to be the unclamped natural width on grab #1 of a
    // natural-width image, freezing the overlay. (#1843 round 5 — Option B fix)
    cachedColumnEl =
      (e.currentTarget as HTMLElement)?.closest?.('.pane-editor') ??
      document.querySelector('.pane-editor') ??
      null;

    // Clamp the anchor at capture time so applyDrag's column clamp doesn't
    // override every mousemove result from the very first pixel of travel.
    // Natural dims remain in node attrs — only the drag interaction is bounded.
    // See docs/spec/attachment-upload-paths.md §Image resize.
    const columnMaxPx = cachedColumnEl
      ? parseFloat(getComputedStyle(cachedColumnEl).width) || Infinity
      : Infinity;

    const rawStartW = currentW || naturalW;
    dragStartW = Math.min(rawStartW, columnMaxPx);
    // Derive dragStartH from the clamped dragStartW so aspect ratio is consistent
    // with what the user sees (proportional clamp → ratio unchanged numerically;
    // explicit for future-proofing if clamping ever becomes non-proportional).
    const rawStartH = currentH || naturalH;
    dragStartH = rawStartW > 0
      ? Math.round(dragStartW * (rawStartH / rawStartW))
      : rawStartH;
    aspectRatio = dragStartH > 0 ? dragStartW / dragStartH : 1;

    if (e instanceof MouseEvent) {
      dragStartX = e.clientX;
      dragStartY = e.clientY;
      // Use document (not window) for global drag tracking — compatible with jsdom test env.
      document.addEventListener('mousemove', onDragMove);
      document.addEventListener('mouseup', onDragEnd);
    } else {
      const touch = e.touches[0];
      dragStartX = touch.clientX;
      dragStartY = touch.clientY;
      document.addEventListener('touchmove', onTouchMove, { passive: false } as AddEventListenerOptions);
      document.addEventListener('touchend', onDragEnd);
    }

    isDragging = true;
    showOverlay = true;
    if (overlayTimer) clearTimeout(overlayTimer);
  }

  function applyDrag(deltaX: number, deltaY: number) {
    // Determine direction multiplier per corner.
    const signX = (dragCorner === 'tl' || dragCorner === 'bl') ? -1 : 1;

    // Aspect-ratio-locked: scale from horizontal delta.
    const candidateW = Math.max(32, dragStartW + signX * deltaX);

    // Clamp to live column width: read once via getComputedStyle on the cached
    // .pane-editor element. Returns the actual rendered pixel width (handles
    // responsive viewports below 720px automatically — no hardcoded 720).
    // Fallback: Infinity (no clamp) when element is not available (jsdom, edge cases).
    const columnMaxPx = cachedColumnEl
      ? parseFloat(getComputedStyle(cachedColumnEl).width) || Infinity
      : Infinity;

    const clampedW = Math.min(candidateW, columnMaxPx);
    currentW = Math.round(clampedW);
    // Preserve aspect ratio when clamping — recompute height from the (possibly clamped) width.
    currentH = Math.max(16, Math.round(currentW / aspectRatio));
  }

  function onDragMove(e: MouseEvent) {
    applyDrag(e.clientX - dragStartX, e.clientY - dragStartY);
  }

  function onTouchMove(e: TouchEvent) {
    e.preventDefault();
    const touch = e.touches[0];
    applyDrag(touch.clientX - dragStartX, touch.clientY - dragStartY);
  }

  function onDragEnd() {
    isDragging = false;
    document.removeEventListener('mousemove', onDragMove);
    document.removeEventListener('mouseup', onDragEnd);
    document.removeEventListener('touchmove', onTouchMove as any);
    document.removeEventListener('touchend', onDragEnd);

    // Persist final dimensions to the node.
    if (onUpdateAttributes) {
      onUpdateAttributes({ width: currentW, height: currentH });
    } else {
      console.warn(
        '[AttachmentImageInline] onUpdateAttributes is undefined — resize dims swallowed. Factory wire broken?',
        { width: currentW, height: currentH, attachmentId }
      );
    }

    // Hide overlay after 500ms.
    overlayTimer = setTimeout(() => {
      showOverlay = false;
    }, 500);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="at-image-inline"
  class:is-selected={selected}
  class:is-dragging={isDragging}
  style={containerStyle}
>
  <!--
    Always render <img> (not conditional on resolvedSrc) so it's always in the
    DOM for tests. src='' before async resolution; non-empty after onMount resolves.
  -->
  <img
    src={resolvedSrc}
    alt={filename}
    style={imageStyle}
    class="at-img"
    onload={handleImageLoad}
    draggable={false}
  />

  <!-- Dimension overlay (shown during drag) -->
  {#if showOverlay && currentW > 0 && currentH > 0}
    <div class="at-dim-overlay">
      {currentW} × {currentH}
    </div>
  {/if}

  <!-- Corner drag handles (always rendered; opacity controlled by CSS) -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="at-handle at-handle-tl"
    onmousedown={(e) => startDrag(e, 'tl')}
    ontouchstart={(e) => startDrag(e, 'tl')}
    role="button"
    tabindex="-1"
    aria-label="Resize from top-left"
  ></div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="at-handle at-handle-tr"
    onmousedown={(e) => startDrag(e, 'tr')}
    ontouchstart={(e) => startDrag(e, 'tr')}
    role="button"
    tabindex="-1"
    aria-label="Resize from top-right"
  ></div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="at-handle at-handle-bl"
    onmousedown={(e) => startDrag(e, 'bl')}
    ontouchstart={(e) => startDrag(e, 'bl')}
    role="button"
    tabindex="-1"
    aria-label="Resize from bottom-left"
  ></div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="at-handle at-handle-br"
    onmousedown={(e) => startDrag(e, 'br')}
    ontouchstart={(e) => startDrag(e, 'br')}
    role="button"
    tabindex="-1"
    aria-label="Resize from bottom-right"
  ></div>
</div>

<style>
  .at-image-inline {
    position: relative;
    display: inline-block;
    line-height: 0;
    cursor: default;
    border-radius: 4px;
  }

  /* Selection ring (matches --accent cyan from AttachmentCard design tokens) */
  .at-image-inline.is-selected {
    box-shadow: 0 0 0 2px var(--accent, #00E4E4);
  }

  .at-image-inline.is-dragging {
    user-select: none;
  }

  .at-img {
    display: block;
    max-width: 100%;
    border-radius: 4px;
    /* Prevent browser's native drag from interfering with our resize drag */
    -webkit-user-drag: none;
    user-select: none;
    /* aspect-ratio fallback: populated once natural dims are known, prevents
       layout stretch during the async gap between mount and image load. */
    aspect-ratio: var(--at-img-ar, auto);
  }

  /* Dimension overlay */
  .at-dim-overlay {
    position: absolute;
    bottom: 8px;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(0, 0, 0, 0.7);
    color: #fff;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    border-radius: 4px;
    pointer-events: none;
    white-space: nowrap;
    z-index: 10;
  }

  /* Corner drag handles */
  .at-handle {
    position: absolute;
    width: 10px;
    height: 10px;
    background: var(--accent, #00E4E4);
    border: 1.5px solid rgba(0, 0, 0, 0.4);
    border-radius: 2px;
    z-index: 5;
    opacity: 0;
    transition: opacity 0.15s;
  }

  /* Show handles on hover or selection */
  .at-image-inline:hover .at-handle,
  .at-image-inline.is-selected .at-handle {
    opacity: 1;
  }

  .at-handle-tl { top: -5px;    left: -5px;   cursor: nwse-resize; }
  .at-handle-tr { top: -5px;    right: -5px;  cursor: nesw-resize; }
  .at-handle-bl { bottom: -5px; left: -5px;   cursor: nesw-resize; }
  .at-handle-br { bottom: -5px; right: -5px;  cursor: nwse-resize; }
</style>
