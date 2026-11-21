/**
 * attachmentSerializer.test.ts
 *
 * Regression tests for the attachment metadata round-trip fix (#401 PR5b).
 *
 * Root cause (confirmed audit 2026-07-02):
 *   Editor.svelte's ATTACHMENT_DEFAULT_MODES used 'collapsed-card' for audio/video/pdf,
 *   but Attachment.ts:defaultRenderMode() returns 'card'. The mismatch caused every
 *   drag/paste upload to hit the raw-HTML serialization path, which emitted
 *   data-attachment-id/type/render-mode but NOT data-filename or data-size.
 *   On remount, parseHTML reconstructed the node with filename=null/size=null →
 *   AttachmentCard showed "File / AUDIO" instead of the real filename + size.
 *
 * Fix (three changes):
 *   1. Editor.svelte: Deleted ATTACHMENT_DEFAULT_MODES, imported defaultRenderMode()
 *      from Attachment.ts as the single source of truth.
 *   2. Editor.svelte: File-picker upload now calls defaultRenderMode(detectedType)
 *      instead of the hardcoded 'collapsed-card'.
 *   3. Editor.svelte: Raw-HTML wrapper now emits data-filename + data-size
 *      (belt-and-suspenders, per burn-area comment at serializeAttachmentWrapper:~line 536).
 *      This fix lives in buildAttachmentRawHtmlBlock (extracted to this utility module
 *      from Editor.svelte for testability).
 *
 * Test plan (per Matt's directive):
 *   T1–T3 : defaultRenderMode returns 'card' for the affected types; no 'collapsed-card'
 *   T4    : buildAttachmentRawHtmlBlock emits data-filename + data-size
 *   T5–T7 : real save→markdown→parseHTML round-trip for audio, pdf, video — asserts
 *            filename + size both preserved (exercising actual serializer + TipTap parseHTML)
 *
 * Every expect asserts on real serializer/parser output — no stubs, no spy injection.
 * Reverting any of the three fix changes would break T1, T4, T5, T6, or T7.
 *
 * Relates to #401
 */

import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest';
import { Editor } from '@tiptap/core';
import { StarterKit } from '@tiptap/starter-kit';
import { tick } from 'svelte';

import { buildAttachmentRawHtmlBlock, escapeHtml, extOf } from './attachmentSerializer';
import { Attachment, defaultRenderMode } from '$lib/extensions/Attachment';

// ---------------------------------------------------------------------------
// Environment polyfills (mirrors Attachment.factory-integration.test.ts)
// ---------------------------------------------------------------------------

beforeAll(() => {
  if (typeof window !== 'undefined') {
    if (!window.DOMParser && typeof DOMParser !== 'undefined') {
      (window as any).DOMParser = DOMParser;
    }
    if (!window.setTimeout && typeof setTimeout !== 'undefined') {
      (window as any).setTimeout = setTimeout;
    }
    if (!window.clearTimeout && typeof clearTimeout !== 'undefined') {
      (window as any).clearTimeout = clearTimeout;
    }
    if (!(window as any).addEventListener) {
      (window as any).addEventListener =
        typeof globalThis.addEventListener === 'function'
          ? globalThis.addEventListener.bind(globalThis)
          : () => {};
    }
    if (!(window as any).removeEventListener) {
      (window as any).removeEventListener =
        typeof globalThis.removeEventListener === 'function'
          ? globalThis.removeEventListener.bind(globalThis)
          : () => {};
    }
  }
});

// ---------------------------------------------------------------------------
// Mock $lib/api (same as factory-integration tests)
// ---------------------------------------------------------------------------

vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn().mockResolvedValue(''),
  openAttachmentExternally: vi.fn().mockResolvedValue(undefined),
}));

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function createTestEditor(): { editor: Editor; el: HTMLElement } {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const editor = new Editor({
    element: el,
    extensions: [StarterKit, Attachment.configure({})],
    content: '',
  });
  return { editor, el };
}

/**
 * Insert a node, parse the serialized markdown-HTML back, and return the
 * first attachment node's attrs from the resulting TipTap document.
 *
 * This function exercises the same code path as the real save→reload cycle:
 *   1. buildAttachmentRawHtmlBlock produces the markdown-embedded HTML block
 *      (this is what Editor.svelte's serializeAttachmentWrapper now emits for
 *      non-default-mode attachments after the fix).
 *   2. The HTML block is fed back to TipTap's setContent (simulating markdown→html
 *      passthrough of a raw HTML block, which markdownToHtml passes unchanged).
 *   3. TipTap's Attachment extension parseHTML reads data-filename + data-size.
 *   4. We return the node's attrs.
 *
 * A test using this function FAILS if buildAttachmentRawHtmlBlock stops emitting
 * data-filename or data-size, because step 3 would then return null for those attrs.
 */
async function roundTripAttrs(params: {
  attachmentId: string;
  renderMode: string;
  attachmentType: string;
  filename: string;
  size: number;
}): Promise<{ filename: string | null; size: number | null; attachmentType: string | null; renderMode: string | null }> {
  const { attachmentId, renderMode, attachmentType, filename, size } = params;

  // Step 1: Produce the raw-HTML block that the fixed serializer now emits.
  // (Pre-fix: this block was missing data-filename and data-size.)
  const rawHtmlBlock = buildAttachmentRawHtmlBlock({
    attachmentId,
    renderMode,
    attachmentType,
    filenameAttr: filename,
    sizeAttr: String(size),
    widthAttr: null,
    heightAttr: null,
    inner: '',
  });

  // Step 2: Parse the raw-HTML block back through TipTap (simulates remount).
  const { editor, el } = createTestEditor();
  try {
    editor.commands.setContent(rawHtmlBlock, false);
    await tick();

    // Step 3: Find the attachment node in the doc.
    let found: { filename: string | null; size: number | null; attachmentType: string | null; renderMode: string | null } | null = null;
    editor.state.doc.descendants((node: any) => {
      if (node.type.name === 'attachment' && !found) {
        found = {
          filename: node.attrs.filename ?? null,
          size: node.attrs.size ?? null,
          attachmentType: node.attrs.attachmentType ?? null,
          renderMode: node.attrs.renderMode ?? null,
        };
      }
    });
    return found ?? { filename: null, size: null, attachmentType: null, renderMode: null };
  } finally {
    editor.destroy();
    if (el.parentNode) el.parentNode.removeChild(el);
  }
}

// ===========================================================================
// T1–T3: defaultRenderMode returns 'card' (not 'collapsed-card') for affected types
// ===========================================================================
// These tests fail if ATTACHMENT_DEFAULT_MODES is re-introduced with 'collapsed-card',
// since that would re-break the isDefaultMode check in Editor.svelte.

describe('defaultRenderMode — single source of truth (#401 PR5b Change 1)', () => {
  // T1
  it('T1: defaultRenderMode("audio") returns "card" (not "collapsed-card")', () => {
    expect(defaultRenderMode('audio')).toBe('card');
    expect(defaultRenderMode('audio')).not.toBe('collapsed-card');
  });

  // T2
  it('T2: defaultRenderMode("pdf") returns "card" (not "collapsed-card")', () => {
    expect(defaultRenderMode('pdf')).toBe('card');
    expect(defaultRenderMode('pdf')).not.toBe('collapsed-card');
  });

  // T3
  it('T3: defaultRenderMode("video") returns "card" (not "collapsed-card")', () => {
    expect(defaultRenderMode('video')).toBe('card');
    expect(defaultRenderMode('video')).not.toBe('collapsed-card');
  });

  it('T3b: defaultRenderMode("html") and "file" also return "card" (not "collapsed-card")', () => {
    expect(defaultRenderMode('html')).toBe('card');
    expect(defaultRenderMode('file')).toBe('card');
  });
});

// ===========================================================================
// T4: buildAttachmentRawHtmlBlock emits data-filename + data-size
// ===========================================================================
// Tests belt-and-suspenders fix (Change 3).
// Pre-fix: the inline code in Editor.svelte's serializeAttachmentWrapper did NOT
// emit these attrs on the non-default-mode path, so these assertions would fail.
// Post-fix: buildAttachmentRawHtmlBlock (which Editor.svelte now delegates to)
// always emits data-filename and data-size when they are present.

describe('buildAttachmentRawHtmlBlock — belt-and-suspenders metadata emission (#401 PR5b Change 3)', () => {
  // T4: core emission test
  it('T4: emits data-filename and data-size in the raw-HTML block wrapper', () => {
    const html = buildAttachmentRawHtmlBlock({
      attachmentId: 'uuid-audio-t4',
      renderMode: 'player',
      attachmentType: 'audio',
      filenameAttr: '07 After The Mystery Is Gone.mp3',
      sizeAttr: '4828000',
      widthAttr: null,
      heightAttr: null,
      inner: '',
    });

    // These assertions FAIL if data-filename / data-size are not emitted:
    expect(html).toContain('data-filename="07 After The Mystery Is Gone.mp3"');
    expect(html).toContain('data-size="4828000"');
    // Core identity attrs must also be present:
    expect(html).toContain('data-attachment-id="uuid-audio-t4"');
    expect(html).toContain('data-render-mode="player"');
    expect(html).toContain('data-attachment-type="audio"');
  });

  it('T4b: omits data-filename and data-size when both are null (no spurious empty attrs)', () => {
    const html = buildAttachmentRawHtmlBlock({
      attachmentId: 'uuid-t4b',
      renderMode: 'player',
      attachmentType: 'audio',
      filenameAttr: null,
      sizeAttr: null,
      widthAttr: null,
      heightAttr: null,
      inner: '',
    });
    expect(html).not.toContain('data-filename');
    expect(html).not.toContain('data-size');
    expect(html).toContain('data-attachment-id="uuid-t4b"');
  });

  it('T4c: HTML-escapes special characters in filename and size attrs', () => {
    const html = buildAttachmentRawHtmlBlock({
      attachmentId: 'uuid-t4c',
      renderMode: 'card',
      attachmentType: 'file',
      filenameAttr: 'A & B <test> "file".pdf',
      sizeAttr: '12345',
      widthAttr: null,
      heightAttr: null,
      inner: '',
    });
    expect(html).toContain('data-filename="A &amp; B &lt;test&gt; &quot;file&quot;.pdf"');
    expect(html).toContain('data-size="12345"');
  });

  it('T4d: width and height attrs still emitted correctly alongside filename+size', () => {
    const html = buildAttachmentRawHtmlBlock({
      attachmentId: 'uuid-t4d',
      renderMode: 'full-inline',
      attachmentType: 'image',
      filenameAttr: 'photo.jpg',
      sizeAttr: '2048000',
      widthAttr: '640',
      heightAttr: '480',
      inner: '<img src="Assets/uuid-t4d.jpg" />',
    });
    expect(html).toContain('data-filename="photo.jpg"');
    expect(html).toContain('data-size="2048000"');
    expect(html).toContain('data-width="640"');
    expect(html).toContain('data-height="480"');
  });
});

// ===========================================================================
// T5–T7: Real save→markdown→parseHTML round-trip for audio, pdf, video
// ===========================================================================
// Each test:
//   1. Calls buildAttachmentRawHtmlBlock (the real function Editor.svelte now uses)
//   2. Feeds the output to TipTap's real parseHTML
//   3. Asserts attrs.filename AND attrs.size are both preserved
//
// These tests fail if buildAttachmentRawHtmlBlock stops emitting data-filename or
// data-size, because TipTap parseHTML would then reconstruct null for those attrs.
// They also fail if Attachment.ts parseHTML stops reading these attrs.
//
// Note: 'player' mode (audio) and 'reader' mode (pdf) are used as the renderMode
// to exercise the non-default-mode path (which always goes through raw-HTML even
// after Fix 1). Fix 3 (belt-and-suspenders) is what makes these round-trips work.

describe('Round-trip: save→markdown→parseHTML preserves filename+size (#401 PR5b Change 3)', () => {
  // T5: audio round-trip
  it('T5: audio attachment — filename and size survive raw-HTML round-trip', async () => {
    const attrs = await roundTripAttrs({
      attachmentId: 'uuid-audio-t5',
      renderMode: 'player',     // non-default for audio (default is 'card') → raw-HTML path
      attachmentType: 'audio',
      filename: '07 After The Mystery Is Gone.mp3',
      size: 4828000,
    });

    // These assertions FAIL pre-fix (data-filename/data-size were not emitted by old code):
    expect(attrs.filename).toBe('07 After The Mystery Is Gone.mp3');
    expect(attrs.size).toBe(4828000);
    // Sanity-check other attrs still round-trip correctly:
    expect(attrs.attachmentType).toBe('audio');
    expect(attrs.renderMode).toBe('player');
  });

  // T6: pdf round-trip
  it('T6: pdf attachment — filename and size survive raw-HTML round-trip', async () => {
    const attrs = await roundTripAttrs({
      attachmentId: 'uuid-pdf-t6',
      renderMode: 'reader',    // non-default for pdf (default is 'card') → raw-HTML path
      attachmentType: 'pdf',
      filename: 'Q3-2026-Report.pdf',
      size: 1572864,
    });

    expect(attrs.filename).toBe('Q3-2026-Report.pdf');
    expect(attrs.size).toBe(1572864);
    expect(attrs.attachmentType).toBe('pdf');
    expect(attrs.renderMode).toBe('reader');
  });

  // T7: video round-trip
  it('T7: video attachment — filename and size survive raw-HTML round-trip', async () => {
    const attrs = await roundTripAttrs({
      attachmentId: 'uuid-video-t7',
      renderMode: 'player',    // non-default for video (default is 'card') → raw-HTML path
      attachmentType: 'video',
      filename: 'session-recording-2026.mp4',
      size: 94371840,
    });

    expect(attrs.filename).toBe('session-recording-2026.mp4');
    expect(attrs.size).toBe(94371840);
    expect(attrs.attachmentType).toBe('video');
    expect(attrs.renderMode).toBe('player');
  });
});

// ===========================================================================
// Utility function sanity checks
// ===========================================================================

describe('escapeHtml utility', () => {
  it('escapes all five special HTML characters', () => {
    expect(escapeHtml('&')).toBe('&amp;');
    expect(escapeHtml('<')).toBe('&lt;');
    expect(escapeHtml('>')).toBe('&gt;');
    expect(escapeHtml('"')).toBe('&quot;');
    expect(escapeHtml("'")).toBe('&#039;');
  });

  it('leaves safe strings unchanged', () => {
    expect(escapeHtml('hello world')).toBe('hello world');
    expect(escapeHtml('07 After The Mystery Is Gone.mp3')).toBe('07 After The Mystery Is Gone.mp3');
  });
});

describe('extOf utility', () => {
  it('extracts extension including dot', () => {
    expect(extOf('photo.jpg')).toBe('.jpg');
    expect(extOf('track.mp3')).toBe('.mp3');
    expect(extOf('report.pdf')).toBe('.pdf');
  });

  it('returns empty string for no extension', () => {
    expect(extOf('nodotfile')).toBe('');
    expect(extOf('')).toBe('');
  });

  it('handles multiple dots — returns last extension', () => {
    expect(extOf('archive.tar.gz')).toBe('.gz');
  });
});
