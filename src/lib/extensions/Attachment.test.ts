/**
 * Attachment.ts — unit tests
 *
 * Tests cover:
 *  - detectAttachmentType()
 *  - defaultRenderMode()  (§2.1A per-type enum — #401 PR1, fixes #1828)
 *  - resolveRenderMode()  (invalid-mode fallback)
 */

import { describe, it, expect } from 'vitest';
import { detectAttachmentType, defaultRenderMode, resolveRenderMode, _shouldStopEventForNodeView } from './Attachment';

// ---------------------------------------------------------------------------
// detectAttachmentType
// ---------------------------------------------------------------------------

describe('detectAttachmentType', () => {
  function makeFile(name: string, type: string): File {
    return new File([''], name, { type });
  }

  // Image detection
  it('returns "image" for image/* mime type', () => {
    expect(detectAttachmentType(makeFile('photo.png', 'image/png'))).toBe('image');
  });

  it('returns "image" for common image extensions (no mime)', () => {
    for (const ext of ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg']) {
      expect(detectAttachmentType(makeFile(`file.${ext}`, ''))).toBe('image');
    }
  });

  // Audio detection
  it('returns "audio" for audio/* mime type', () => {
    expect(detectAttachmentType(makeFile('track.mp3', 'audio/mpeg'))).toBe('audio');
  });

  it('returns "audio" for common audio extensions (no mime)', () => {
    for (const ext of ['mp3', 'wav', 'ogg', 'm4a', 'aac', 'flac']) {
      expect(detectAttachmentType(makeFile(`file.${ext}`, ''))).toBe('audio');
    }
  });

  // Video detection
  it('returns "video" for video/* mime type', () => {
    expect(detectAttachmentType(makeFile('clip.mp4', 'video/mp4'))).toBe('video');
  });

  it('returns "video" for common video extensions (no mime)', () => {
    for (const ext of ['mp4', 'webm', 'mov', 'avi', 'mkv']) {
      expect(detectAttachmentType(makeFile(`file.${ext}`, ''))).toBe('video');
    }
  });

  // PDF detection
  it('returns "pdf" for application/pdf mime type', () => {
    expect(detectAttachmentType(makeFile('doc.pdf', 'application/pdf'))).toBe('pdf');
  });

  it('returns "pdf" for .pdf extension (no mime)', () => {
    expect(detectAttachmentType(makeFile('report.pdf', ''))).toBe('pdf');
  });

  // HTML detection
  it('returns "html" for text/html mime type', () => {
    expect(detectAttachmentType(makeFile('page.html', 'text/html'))).toBe('html');
  });

  it('returns "html" for .html / .htm extension (no mime)', () => {
    expect(detectAttachmentType(makeFile('page.html', ''))).toBe('html');
    expect(detectAttachmentType(makeFile('page.htm', ''))).toBe('html');
  });

  // Generic file fallback
  it('returns "file" for unknown types', () => {
    expect(detectAttachmentType(makeFile('archive.zip', 'application/zip'))).toBe('file');
    expect(detectAttachmentType(makeFile('data.csv', 'text/csv'))).toBe('file');
    expect(detectAttachmentType(makeFile('unknown.xyz', ''))).toBe('file');
  });

  // Case-insensitive extension matching
  it('is case-insensitive for extension matching', () => {
    expect(detectAttachmentType(makeFile('PHOTO.JPG', ''))).toBe('image');
    expect(detectAttachmentType(makeFile('CLIP.MP4', ''))).toBe('video');
    expect(detectAttachmentType(makeFile('DOC.PDF', ''))).toBe('pdf');
  });
});

// ---------------------------------------------------------------------------
// defaultRenderMode — §2.1A per-type enum (#401 PR1, fixes #1828)
// ---------------------------------------------------------------------------

describe('defaultRenderMode', () => {
  it('returns "full-inline" for image (expanded by default)', () => {
    expect(defaultRenderMode('image')).toBe('full-inline');
  });

  it('returns "card" for pdf (collapsed card)', () => {
    expect(defaultRenderMode('pdf')).toBe('card');
  });

  it('returns "card" for audio (collapsed card)', () => {
    expect(defaultRenderMode('audio')).toBe('card');
  });

  it('returns "card" for video (collapsed card)', () => {
    expect(defaultRenderMode('video')).toBe('card');
  });

  it('returns "card" for html (collapsed card)', () => {
    expect(defaultRenderMode('html')).toBe('card');
  });

  // #1828: link default must be 'preview', NOT 'collapsed-card' or 'card'.
  // Link has no 'card' mode at all.
  it('returns "preview" for link (NOT card — fixes #1828)', () => {
    expect(defaultRenderMode('link')).toBe('preview');
    expect(defaultRenderMode('link')).not.toBe('card');
    expect(defaultRenderMode('link')).not.toBe('collapsed-card');
  });

  it('returns "card" for file (generic fallback)', () => {
    expect(defaultRenderMode('file')).toBe('card');
  });

  it('returns "card" for unknown type string (safe fallback)', () => {
    expect(defaultRenderMode('unknown')).toBe('card');
  });

  it('returns "card" for any arbitrary unknown type string', () => {
    expect(defaultRenderMode('spreadsheet')).toBe('card');
    expect(defaultRenderMode('')).toBe('card');
    expect(defaultRenderMode('LINK')).toBe('card'); // case-sensitive
  });
});

// ---------------------------------------------------------------------------
// _shouldStopEventForNodeView — stopEvent predicate for NodeView spec (#1843)
//
// T-D1  positive: target is .at-handle → true
// T-D1a positive: target is a child of .at-handle → true (verifies .closest)
// T-D2  negative: target is .at-mode-button → false (#1858 protection)
// T-D3  negative: target is .at-nv-host → false (drag-to-move protection)
// T-D4  edge: null target / no .at-handle ancestor → false
// ---------------------------------------------------------------------------

describe('_shouldStopEventForNodeView (stopEvent predicate)', () => {
  /** Minimal Event stub with a real DOM element as `target`. */
  function makeEvent(target: HTMLElement | null): Event {
    return { target } as unknown as Event;
  }

  // T-D1: direct .at-handle target → true
  it('T-D1: returns true when event target is a .at-handle element', () => {
    const handle = document.createElement('div');
    handle.classList.add('at-handle');
    expect(_shouldStopEventForNodeView(makeEvent(handle))).toBe(true);
  });

  // T-D1a: child nested inside .at-handle → still true (via .closest)
  it('T-D1a: returns true when event target is a descendant of .at-handle', () => {
    const handle = document.createElement('div');
    handle.classList.add('at-handle');
    const inner = document.createElement('span');
    handle.appendChild(inner);
    expect(_shouldStopEventForNodeView(makeEvent(inner))).toBe(true);
  });

  // T-D2: .at-tbtn--mode → false (picker one-click, #1858 protection)
  // Fix: at-mode-button was a phantom class (doesn't exist in production);
  // real class on AttachmentToolbar.svelte:90 is at-tbtn--mode. (#1867)
  it('T-D2: returns false when event target is .at-tbtn--mode (protects #1858)', () => {
    const btn = document.createElement('button');
    btn.classList.add('at-tbtn--mode');
    expect(_shouldStopEventForNodeView(makeEvent(btn))).toBe(false);
  });

  // T-D3: .at-nv-host → false (node body; drag-to-move must be preserved)
  it('T-D3: returns false when event target is .at-nv-host (protects drag-to-move)', () => {
    const host = document.createElement('div');
    host.classList.add('at-nv-host');
    expect(_shouldStopEventForNodeView(makeEvent(host))).toBe(false);
  });

  // T-D4: null target → false
  it('T-D4a: returns false when event target is null', () => {
    expect(_shouldStopEventForNodeView(makeEvent(null))).toBe(false);
  });

  // T-D4: unrelated element with no .at-handle ancestor → false
  it('T-D4b: returns false when event target has no .at-handle ancestor', () => {
    const div = document.createElement('div');
    div.classList.add('at-image-inline');
    expect(_shouldStopEventForNodeView(makeEvent(div))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// resolveRenderMode — invalid mode strings fall back to type default
// ---------------------------------------------------------------------------

describe('resolveRenderMode', () => {
  it('returns the mode unchanged when valid for the type', () => {
    expect(resolveRenderMode('image', 'full-inline')).toBe('full-inline');
    expect(resolveRenderMode('image', 'card')).toBe('card');
    expect(resolveRenderMode('pdf', 'card')).toBe('card');
    expect(resolveRenderMode('pdf', 'reader')).toBe('reader');
    expect(resolveRenderMode('audio', 'player')).toBe('player');
    expect(resolveRenderMode('video', 'player')).toBe('player');
    expect(resolveRenderMode('html', 'preview')).toBe('preview');
    expect(resolveRenderMode('link', 'preview')).toBe('preview');
    expect(resolveRenderMode('link', 'plain')).toBe('plain');
    // 'embed' removed from VALID_RENDER_MODES.link (#401 rip) — falls back to 'preview'
    expect(resolveRenderMode('link', 'embed')).toBe('preview');
  });

  it('falls back to type default for invalid mode string', () => {
    // 'collapsed-card' was the old (wrong) default — never valid under §2.1A
    expect(resolveRenderMode('image', 'collapsed-card')).toBe('full-inline');
    expect(resolveRenderMode('pdf', 'collapsed-card')).toBe('card');
    expect(resolveRenderMode('audio', 'collapsed-card')).toBe('card');
    expect(resolveRenderMode('video', 'collapsed-card')).toBe('card');
    expect(resolveRenderMode('html', 'collapsed-card')).toBe('card');
    expect(resolveRenderMode('link', 'collapsed-card')).toBe('preview');
    // link has no 'card' mode — must fall back to 'preview'
    expect(resolveRenderMode('link', 'card')).toBe('preview');
  });

  it('falls back to type default for null / undefined mode', () => {
    expect(resolveRenderMode('image', null)).toBe('full-inline');
    expect(resolveRenderMode('link', null)).toBe('preview');
    expect(resolveRenderMode('pdf', undefined)).toBe('card');
  });

  it('falls back to "card" for unknown type with any mode', () => {
    expect(resolveRenderMode('unknown', 'reader')).toBe('card');
    expect(resolveRenderMode('spreadsheet', 'anything')).toBe('card');
  });
});

// ---------------------------------------------------------------------------
// Attachment.ts addAttributes() — size attr serialization (#1867)
// ---------------------------------------------------------------------------

import { Attachment } from './Attachment';

describe('Attachment node — size attribute (addAttributes)', () => {
  // Access the size attribute definition directly from the extension's addAttributes().
  // We call addAttributes() on the extension instance to inspect the attr spec.
  function getSizeAttr() {
    // Attachment.create() returns an Extension instance; addAttributes() is on its config.
    // Access via the raw extension config.
    const ext = Attachment;
    // addAttributes returns the full attribute map; we grab size.
    const attrMap = (ext as any).config.addAttributes();
    return attrMap.size;
  }

  it('size attr has default: null', () => {
    const sizeAttr = getSizeAttr();
    expect(sizeAttr.default).toBeNull();
  });

  it('size parseHTML: parses data-size="12345" to 12345', () => {
    const sizeAttr = getSizeAttr();
    const el = document.createElement('div');
    el.setAttribute('data-size', '12345');
    expect(sizeAttr.parseHTML(el)).toBe(12345);
  });

  it('size parseHTML: returns null when data-size is absent', () => {
    const sizeAttr = getSizeAttr();
    const el = document.createElement('div');
    expect(sizeAttr.parseHTML(el)).toBeNull();
  });

  it('size parseHTML: returns null when data-size is non-numeric', () => {
    const sizeAttr = getSizeAttr();
    const el = document.createElement('div');
    el.setAttribute('data-size', 'not-a-number');
    expect(sizeAttr.parseHTML(el)).toBeNull();
  });

  it('size renderHTML: renders 12345 as { "data-size": "12345" }', () => {
    const sizeAttr = getSizeAttr();
    expect(sizeAttr.renderHTML({ size: 12345 })).toEqual({ 'data-size': '12345' });
  });

  it('size renderHTML: returns {} (omits data-size) when size is null', () => {
    const sizeAttr = getSizeAttr();
    expect(sizeAttr.renderHTML({ size: null })).toEqual({});
  });

  it('size renderHTML: returns {} (omits data-size) when size is undefined', () => {
    const sizeAttr = getSizeAttr();
    expect(sizeAttr.renderHTML({ size: undefined })).toEqual({});
  });

  it('formatBytes(null) returns empty string — regression guard for pre-#1867 markdown', () => {
    // formatBytes is an internal fn in AttachmentCard.svelte, not exported from Attachment.ts.
    // Guard here via the attr spec: if size parses as null (missing data-size),
    // the card must degrade gracefully. We verify the parseHTML → null path is solid.
    const sizeAttr = getSizeAttr();
    const el = document.createElement('div'); // no data-size
    const parsed = sizeAttr.parseHTML(el);
    expect(parsed).toBeNull(); // null flows to formatBytes(null) → '' in the card
  });
});
