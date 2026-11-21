<script lang="ts">
  /**
   * AttachmentHTMLPreview.svelte — Sandboxed HTML preview for PR6b (#401).
   *
   * Sole HTML render path. Content flows: bytes → utf8 string → CSP-wrapped →
   * $state srcdocContent → srcdoc={} binding. Never enters contenteditable DOM.
   *
   * Security invariants (per ADR-0049 Amendment B §96):
   *  - <iframe sandbox=""> — empty always. No script allowances, no same-origin allowance.
   *  - Content delivered via srcdoc only — never data: src (blocked by app CSP).
   *  - Document-level CSP meta prepended: default-src 'none'; style-src 'unsafe-inline'; img-src data:
   *  - Raw HTML never passed through HTML injection — Svelte srcdoc binding is not DOM injection.
   *
   * Size caps:
   *  - >5 MB hard cap: refuse to render.
   *  - >1 MB soft cap: show affordance with "Preview anyway" option.
   *
   * Link egress (openExternal) deferred to PR7 (#1780). TODO stub only.
   *
   * Mirrors AttachmentPdfReader.svelte lifecycle and structure.
   */

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  // ─── CSP wrapper constants ────────────────────────────────────────────────────

  const CSP_META = '<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'; img-src data:">';
  const BASE_TAG = '<base target="_blank">'; // Link-egress deferred to PR7; target=_blank no-ops in empty sandbox

  function wrapWithCsp(rawHtml: string): string {
    return `<!DOCTYPE html>\n${CSP_META}\n${BASE_TAG}\n${rawHtml}`;
  }

  function formatSize(bytes: number): string {
    const mb = bytes / (1024 * 1024);
    return mb < 0.1 ? `${(bytes / 1024).toFixed(0)} KB` : `${mb.toFixed(1)} MB`;
  }

  // ─── Props ────────────────────────────────────────────────────────────────────

  interface Props {
    attachmentId: string;
    filename: string;
  }

  let { attachmentId, filename }: Props = $props();

  // ─── State ────────────────────────────────────────────────────────────────────

  let srcdocContent: string = $state('');
  let error: string | null = $state(null);
  let sizeBytes: number = $state(0);

  // ─── Helpers ─────────────────────────────────────────────────────────────────

  function openExternal() {
    // TODO: PR7 (#1780) will implement external open via shell.open or equivalent.
    console.warn('PR7 will implement external open');
  }

  function renderContent(rawHtml: string) {
    srcdocContent = wrapWithCsp(rawHtml);
  }

  // ─── Lifecycle ────────────────────────────────────────────────────────────────

  onMount(async () => {
    try {
      const result = await invoke<{ id: string; mime_type: string; original_filename: string; data: string }>(
        'get_attachment_data',
        { attachmentId }
      );

      // Decode base64 → bytes → utf8 string
      const binaryStr = atob(result.data);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }
      sizeBytes = bytes.length;

      const rawHtml = new TextDecoder('utf-8').decode(bytes);

      const HARD_CAP = 5 * 1024 * 1024; // 5 MB
      const SOFT_CAP = 1 * 1024 * 1024; // 1 MB

      if (sizeBytes > HARD_CAP) {
        error = 'too-large-hard';
        return;
      }

      if (sizeBytes > SOFT_CAP) {
        error = 'too-large-soft';
        // Store raw HTML for "Preview anyway" — will be wrapped on demand
        srcdocContent = rawHtml; // temporarily store raw; renderContent called on confirm
        return;
      }

      renderContent(rawHtml);
    } catch (e) {
      error = 'load-failed';
    }
  });
</script>

<div class="at-html-preview">
  {#if error === 'too-large-hard'}
    <div class="at-html-error">
      File too large to preview ({formatSize(sizeBytes)}).
      <button onclick={openExternal}>Open externally</button>
    </div>
  {:else if error === 'too-large-soft'}
    <div class="at-html-error">
      Large file ({formatSize(sizeBytes)}).
      <button onclick={openExternal}>Open externally</button>
      <button onclick={() => { const raw = srcdocContent; error = null; renderContent(raw); }}>Preview anyway</button>
    </div>
  {:else if error === 'load-failed'}
    <div class="at-html-error">Failed to load HTML content.</div>
  {:else if srcdocContent}
    <iframe
      class="at-html-frame"
      sandbox=""
      srcdoc={srcdocContent}
      title={filename}
    ></iframe>
  {:else}
    <div class="at-html-loading" aria-label="Loading HTML preview"></div>
  {/if}
</div>

<style>
  .at-html-preview {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-height: 400px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-secondary, #1a1a1a);
  }

  .at-html-frame {
    border: 0;
    width: 100%;
    height: 100%;
    min-height: 400px;
    flex: 1;
    display: block;
  }

  .at-html-error {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    color: var(--text-secondary, #888);
    font-size: 14px;
    flex-wrap: wrap;
  }

  .at-html-error button {
    padding: 4px 10px;
    border-radius: 4px;
    border: 1px solid var(--border, #333);
    background: var(--surface-secondary, #222);
    color: var(--text-primary, #fff);
    font-size: 13px;
    cursor: pointer;
  }

  .at-html-loading {
    flex: 1;
    min-height: 400px;
    background: var(--surface-secondary, #1a1a1a);
  }
</style>
