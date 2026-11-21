<script lang="ts">
  /**
   * AttachmentPdfReader.svelte — Inline PDF reader for PR6a (#401).
   *
   * Features:
   *  - Portrait aspect-ratio container (CSS, not fixed px height).
   *    Choice: aspect-ratio: 8.5 / 11 (US Letter portrait). This mirrors the
   *    structural approach of AttachmentVideoPlayer.svelte's 16:9 container but
   *    uses a portrait ratio appropriate for PDFs. The container fills 100% width
   *    and sizes height from the ratio, so PDFs sit naturally in the note flow.
   *  - Native <object type="application/pdf"> — no PDF.js, no worker, no dep.
   *    WKWebView v0.67.84 renders PDF with its own native UI (scroll, zoom, etc.).
   *    We leave that UI alone — do NOT add controls.
   *  - Src resolved via resolveAttachmentSrc(attachmentId) → asset:// URL. Never base64.
   *  - Conditional render: <object> only when resolvedSrc is truthy (loading/failed → placeholder).
   *
   * Scope: PDF reader inline only. Card fallback remains in AttachmentNodeView for
   * pdf in card mode. No toolbar, no mode-change buttons (handled by AttachmentNodeView).
   *
   * Mirrors AttachmentVideoPlayer.svelte lifecycle and structure exactly.
   */

  import { onMount } from 'svelte';
  import { resolveAttachmentSrc } from '$lib/api';

  interface Props {
    attachmentId: string;
    filename: string;
  }

  let { attachmentId, filename }: Props = $props();

  let resolvedSrc: string | null = $state(null);

  onMount(async () => {
    resolvedSrc = await resolveAttachmentSrc(attachmentId);
  });
</script>

<div class="at-pdf-reader">
  <div class="at-pdf-ratio">
    {#if resolvedSrc}
      <!--
        Native <object> — WKWebView renders PDF with its own UI (scroll, zoom).
        Do NOT add controls. Leave it alone.
        data: bound to resolvedSrc (asset:// URL from resolveAttachmentSrc).
      -->
      <object
        class="at-pdf-object"
        data={resolvedSrc}
        type="application/pdf"
        width="100%"
        height="100%"
        title={filename}
      >
        <!-- Inline fallback text for browsers that do not support object rendering. -->
        Unable to display PDF.
      </object>
    {:else}
      <!-- Loading / failed placeholder: minimal, mirrors VideoPlayer empty-state. -->
      <div class="at-pdf-placeholder" aria-label="Loading PDF…"></div>
    {/if}
  </div>
</div>

<style>
  /* Outer block */
  .at-pdf-reader {
    display: block;
    width: 100%;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-secondary, #1a1a1a);
  }

  /*
   * Portrait aspect-ratio container: 8.5 / 11 (US Letter).
   * Mirrors the structural approach of AttachmentVideoPlayer's .at-video-ratio (16/9)
   * but portrait-oriented for PDFs. CSS aspect-ratio keeps height proportional to width.
   */
  .at-pdf-ratio {
    position: relative;
    width: 100%;
    aspect-ratio: 8.5 / 11;
    background: var(--surface-secondary, #1a1a1a);
  }

  .at-pdf-object {
    position: absolute;
    inset: 0;
    display: block;
    background: var(--surface-secondary, #1a1a1a);
  }

  /* Placeholder: loading or failed state. Empty div, minimal UI. */
  .at-pdf-placeholder {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--surface-secondary, #1a1a1a);
  }
</style>
