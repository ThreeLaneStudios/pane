/**
 * Editor.svelte — htmlToMarkdown attachment serializer tests (PR2b)
 *
 * Tests the serializeAttachmentWrapper + computeAttachmentPrefix logic in
 * isolation by constructing DOM elements that match what Attachment.ts
 * renderHTML produces.
 *
 * Covers:
 *  - Default mode → relative Assets/ paths (spec §2.1 canonical format)
 *  - Non-default mode → raw-HTML block with relative Assets/ paths
 *  - Legacy data-id backward compat
 *  - data-attachment-id (canonical form)
 *  - Depth calculation (root, one deep, two deep)
 *  - markdownToHtml: parse canonical Assets/ paths (backward-compat + new form)
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';

// ---------------------------------------------------------------------------
// Inline helpers (mirrors the logic from Editor.svelte; JSDOM provides the DOM)
// ---------------------------------------------------------------------------

const ATTACHMENT_DEFAULT_MODES: Record<string, string> = {
  image: 'full-inline',
  audio: 'collapsed-card',
  video: 'collapsed-card',
  pdf: 'collapsed-card',
  html: 'collapsed-card',
  link: 'collapsed-card',
  file: 'collapsed-card',
};

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

function extOf(filename: string): string {
  const dot = filename.lastIndexOf('.');
  return dot >= 0 ? filename.substring(dot) : '';
}

/** Mirrors Editor.svelte computeAttachmentPrefix */
function computeAttachmentPrefix(filePath: string | undefined): string {
  if (!filePath) return '';
  const parts = filePath.replace(/\\/g, '/').split('/');
  const depth = Math.max(0, parts.length - 3);
  return '../'.repeat(depth);
}

function serializeAttachmentWrapper(el: HTMLElement, filePath?: string): string {
  const attachmentId =
    el.getAttribute('data-attachment-id') ||
    el.getAttribute('data-id') ||
    el.querySelector('[data-attachment-id]')?.getAttribute('data-attachment-id') ||
    el.querySelector('[data-id]')?.getAttribute('data-id') ||
    null;

  const renderMode = el.getAttribute('data-render-mode') || null;
  const attachmentType = el.getAttribute('data-attachment-type') || null;

  const isDefaultMode =
    !renderMode ||
    (attachmentType != null && ATTACHMENT_DEFAULT_MODES[attachmentType] === renderMode);

  const prefix = computeAttachmentPrefix(filePath);

  if (!isDefaultMode && attachmentId && renderMode && attachmentType) {
    const img = el.querySelector('img');
    const audioEl = el.querySelector('audio');
    const videoEl = el.querySelector('video');
    let filenameForExt = '';
    if (img) {
      filenameForExt = img.getAttribute('data-filename') || img.getAttribute('alt') || '';
    } else {
      filenameForExt = el.querySelector('.attachment-filename')?.textContent ||
        el.getAttribute('data-filename') || '';
    }
    const relPath = `${prefix}Assets/${attachmentId}${extOf(filenameForExt)}`;
    let inner = el.innerHTML;
    inner = inner.replace(new RegExp(`attachment://${attachmentId}`, 'g'), relPath);
    return (
      `<div data-attachment data-attachment-id="${escapeHtml(attachmentId)}"` +
      ` data-render-mode="${escapeHtml(renderMode)}"` +
      ` data-attachment-type="${escapeHtml(attachmentType)}">${inner}</div>\n\n`
    );
  }

  if (!attachmentId) return '';

  const img = el.querySelector('img');
  if (img) {
    const filename = img.getAttribute('data-filename') || img.getAttribute('alt') || 'image';
    const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
    return `![${filename}](${relPath})\n\n`;
  }

  const audio = el.querySelector('audio');
  if (audio) {
    const filename = el.querySelector('.attachment-filename')?.textContent || 'audio';
    const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
    return `[${filename}](${relPath})\n\n`;
  }

  const video = el.querySelector('video');
  if (video) {
    const filename = el.querySelector('.attachment-filename')?.textContent || 'video';
    const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
    return `[${filename}](${relPath})\n\n`;
  }

  const filename = el.querySelector('.attachment-filename')?.textContent ||
    el.getAttribute('data-filename') || 'file';
  const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
  return `[${filename}](${relPath})\n\n`;
}

// ---------------------------------------------------------------------------
// Setup — use jsdom for DOM APIs
// ---------------------------------------------------------------------------

let document: Document;

beforeAll(() => {
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  document = dom.window.document;
});

function makeWrapper(html: string): HTMLElement {
  const div = document.createElement('div');
  div.innerHTML = html;
  return div.firstElementChild as HTMLElement;
}

// ---------------------------------------------------------------------------
// Depth calculation
// ---------------------------------------------------------------------------

describe('computeAttachmentPrefix', () => {
  it('returns "" for a note at project root (depth 0)', () => {
    expect(computeAttachmentPrefix('Projects/Foo/note.md')).toBe('');
  });

  it('returns "../" for a note one folder deep (depth 1)', () => {
    expect(computeAttachmentPrefix('Projects/Foo/Subfolder/note.md')).toBe('../');
  });

  it('returns "../../" for a note two folders deep (depth 2)', () => {
    expect(computeAttachmentPrefix('Projects/Foo/Sub/Deep/note.md')).toBe('../../');
  });

  it('returns "" when file_path is undefined', () => {
    expect(computeAttachmentPrefix(undefined)).toBe('');
  });

  it('handles Windows-style backslash paths', () => {
    expect(computeAttachmentPrefix('Projects\\Foo\\note.md')).toBe('');
    expect(computeAttachmentPrefix('Projects\\Foo\\Sub\\note.md')).toBe('../');
  });
});

// ---------------------------------------------------------------------------
// Default mode → relative Assets/ paths
// ---------------------------------------------------------------------------

describe('serializeAttachmentWrapper — default mode → relative Assets/ path', () => {
  it('image at root depth → Assets/<uuid>.jpg', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="abc-123"
           data-render-mode="full-inline" data-attachment-type="image">
         <img src="Assets/abc-123.jpg" alt="photo" data-filename="photo.jpg" data-attachment-id="abc-123">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('![photo.jpg](Assets/abc-123.jpg)\n\n');
  });

  it('image one folder deep → ../Assets/<uuid>.jpg', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="abc-123"
           data-render-mode="full-inline" data-attachment-type="image">
         <img src="Assets/abc-123.jpg" alt="photo" data-filename="photo.jpg" data-attachment-id="abc-123">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/Subfolder/note.md');
    expect(result).toBe('![photo.jpg](../Assets/abc-123.jpg)\n\n');
  });

  it('image two folders deep → ../../Assets/<uuid>.jpg', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="abc-123"
           data-render-mode="full-inline" data-attachment-type="image">
         <img src="Assets/abc-123.jpg" alt="photo" data-filename="photo.jpg" data-attachment-id="abc-123">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/Sub/Deep/note.md');
    expect(result).toBe('![photo.jpg](../../Assets/abc-123.jpg)\n\n');
  });

  it('audio with collapsed-card (default) → relative markdown link', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper attachment-audio" data-attachment data-attachment-id="def-456"
           data-render-mode="collapsed-card" data-attachment-type="audio">
         <div class="attachment-filename">song.mp3</div>
         <audio src="Assets/def-456.mp3" controls></audio>
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('[song.mp3](Assets/def-456.mp3)\n\n');
  });

  it('file with collapsed-card (default) → relative markdown link', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper attachment-file" data-attachment data-attachment-id="ghi-789"
           data-render-mode="collapsed-card" data-attachment-type="file" data-filename="report.zip">
         <div class="attachment-filename">report.zip</div>
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('[report.zip](Assets/ghi-789.zip)\n\n');
  });

  it('no renderMode set → treated as default → relative path', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="xyz-999">
         <img src="Assets/xyz-999.png" alt="pic" data-filename="pic.png" data-attachment-id="xyz-999">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('![pic.png](Assets/xyz-999.png)\n\n');
  });
});

// ---------------------------------------------------------------------------
// Non-default mode → raw-HTML block with relative src
// ---------------------------------------------------------------------------

describe('serializeAttachmentWrapper — non-default mode → raw-HTML with relative src', () => {
  it('image with collapsed-card (non-default) → raw-HTML block, attachment:// replaced', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="img-001"
           data-render-mode="collapsed-card" data-attachment-type="image">
         <img src="attachment://img-001" alt="photo" data-filename="photo.jpg" data-attachment-id="img-001">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toContain('data-attachment');
    expect(result).toContain('data-attachment-id="img-001"');
    expect(result).toContain('data-render-mode="collapsed-card"');
    expect(result).toContain('data-attachment-type="image"');
    // attachment:// replaced with relative path
    expect(result).not.toContain('attachment://img-001');
    expect(result).toContain('Assets/img-001.jpg');
    expect(result).toMatch(/^<div /);
    expect(result).toMatch(/<\/div>\n\n$/);
  });

  it('non-default mode one folder deep uses ../Assets/ prefix', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="img-002"
           data-render-mode="collapsed-card" data-attachment-type="image">
         <img src="attachment://img-002" alt="photo" data-filename="shot.png" data-attachment-id="img-002">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/Sub/note.md');
    expect(result).toContain('../Assets/img-002.png');
    expect(result).not.toContain('attachment://img-002');
  });

  it('audio with player mode (non-default) → raw-HTML block', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper attachment-audio" data-attachment data-attachment-id="aud-002"
           data-render-mode="player" data-attachment-type="audio">
         <div class="attachment-filename">track.mp3</div>
         <audio controls></audio>
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toContain('data-attachment-id="aud-002"');
    expect(result).toContain('data-render-mode="player"');
    expect(result).toContain('data-attachment-type="audio"');
    expect(result).not.toMatch(/^\[/); // not a markdown link
  });
});

// ---------------------------------------------------------------------------
// Backward compatibility — legacy data-id
// ---------------------------------------------------------------------------

describe('serializeAttachmentWrapper — backward compat (data-id)', () => {
  it('reads legacy data-id from wrapper div', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-id="old-legacy-id">
         <img src="attachment://old-legacy-id" alt="old" data-filename="old.jpg" data-id="old-legacy-id">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('![old.jpg](Assets/old-legacy-id.jpg)\n\n');
  });

  it('reads legacy data-id from img child when not on wrapper', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper">
         <img src="attachment://child-id" alt="legacy" data-filename="legacy.png" data-id="child-id">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('![legacy.png](Assets/child-id.png)\n\n');
  });

  it('prefers data-attachment-id over legacy data-id', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="new-id" data-id="old-id">
         <img src="attachment://new-id" alt="test" data-filename="test.jpg" data-attachment-id="new-id">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toContain('new-id');
    expect(result).not.toContain('old-id');
  });
});

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

describe('serializeAttachmentWrapper — edge cases', () => {
  it('returns empty string when no attachmentId found', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment></div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('');
  });

  it('image with no extension in filename → no ext in path', () => {
    const el = makeWrapper(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="no-ext-id">
         <img src="attachment://no-ext-id" alt="noext" data-filename="noext" data-attachment-id="no-ext-id">
       </div>`
    );
    const result = serializeAttachmentWrapper(el, 'Projects/Foo/note.md');
    expect(result).toBe('![noext](Assets/no-ext-id)\n\n');
  });
});

// ---------------------------------------------------------------------------
// markdownToHtml — parse both attachment:// and relative Assets/ forms
// ---------------------------------------------------------------------------

/**
 * Minimal inline version of processInline that mirrors Editor.svelte logic.
 * Only tests the attachment-relevant branches.
 */
function processInline(text: string): string {
  // attachment:// image form (legacy)
  text = text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (match, alt, src) => {
    if (src.startsWith('attachment://')) {
      const attachmentId = src.replace('attachment://', '');
      return `<img src="${escapeHtml(src)}" alt="${escapeHtml(alt)}" data-attachment-id="${escapeHtml(attachmentId)}" data-filename="${escapeHtml(alt)}">`;
    }
    // Canonical relative Assets/ form
    const assetsMatch = src.match(/^(?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)$/);
    if (assetsMatch) {
      const attachmentId = assetsMatch[1];
      const ext = assetsMatch[2];
      const safeFilename = alt ? escapeHtml(alt) : escapeHtml(attachmentId + ext);
      return `<img src="${escapeHtml(src)}" alt="${escapeHtml(alt)}" data-attachment-id="${escapeHtml(attachmentId)}" data-filename="${safeFilename}">`;
    }
    return `<img src="${escapeHtml(src)}" alt="${escapeHtml(alt)}">`;
  });

  // attachment:// non-image form (legacy)
  // C3: wrapper divs now carry data-attachment sentinel + data-filename + data-attachment-type
  // so TipTap parseHTML can reconstruct the node with correct attrs on reload.
  text = text.replace(/\[([^\]]*)\]\(attachment:\/\/([^)]+)\)/g, (match, filename, attachmentId) => {
    const safeName = escapeHtml(filename);
    const safeId = escapeHtml(attachmentId);
    const ext = filename.split('.').pop()?.toLowerCase() || '';
    if (['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'].includes(ext)) {
      return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="audio"><div class="attachment-filename">${safeName}</div><audio data-attachment-id="${safeId}" controls></audio></div>`;
    }
    const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'];
    const attachType = ext === 'pdf' ? 'pdf' : imageExts.includes(ext) ? 'image' : 'file';
    return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="${attachType}"><div class="attachment-card"><span class="attachment-filename">${safeName}</span><a class="attachment-download" data-attachment-id="${safeId}">Download</a></div></div>`;
  });

  // Canonical relative Assets/ non-image form
  text = text.replace(/\[([^\]]*)\]\((?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)\)/g, (match, filename, attachmentId, ext) => {
    const safeName = escapeHtml(filename);
    const safeId = escapeHtml(attachmentId);
    const extLower = ext.toLowerCase().slice(1);
    if (['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'].includes(extLower)) {
      return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="audio"><div class="attachment-filename">${safeName}</div><audio data-attachment-id="${safeId}" controls></audio></div>`;
    }
    const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'];
    const attachType = extLower === 'pdf' ? 'pdf' : imageExts.includes(extLower) ? 'image' : 'file';
    return `<div class="attachment-wrapper" data-attachment="" data-attachment-id="${safeId}" data-filename="${safeName}" data-attachment-type="${attachType}"><div class="attachment-card"><span class="attachment-filename">${safeName}</span><a class="attachment-download" data-attachment-id="${safeId}">Download</a></div></div>`;
  });

  return text;
}

describe('processInline — backward compat: attachment:// still parses', () => {
  it('parses legacy image attachment:// link', () => {
    const result = processInline('![photo.jpg](attachment://abc-123)');
    expect(result).toContain('data-attachment-id="abc-123"');
    expect(result).toContain('data-filename="photo.jpg"');
    expect(result).toContain('<img ');
  });

  it('parses legacy audio attachment:// link — stamps data-attachment-type and data-filename', () => {
    const result = processInline('[track.mp3](attachment://def-456)');
    expect(result).toContain('data-attachment-id="def-456"');
    expect(result).toContain('data-attachment-type="audio"');
    expect(result).toContain('data-filename="track.mp3"');
    expect(result).toContain('data-attachment=""');
    expect(result).toContain('attachment-filename');
    expect(result).toContain('<audio ');
  });

  it('parses legacy file attachment:// link — stamps data-attachment-type="pdf" for .pdf', () => {
    const result = processInline('[report.pdf](attachment://ghi-789)');
    expect(result).toContain('data-attachment-id="ghi-789"');
    expect(result).toContain('data-attachment-type="pdf"');
    expect(result).toContain('data-filename="report.pdf"');
    expect(result).toContain('data-attachment=""');
    expect(result).toContain('attachment-filename');
    expect(result).toContain('<div class="attachment-wrapper"');
  });

  it('parses legacy zip attachment:// link — stamps data-attachment-type="file" for unknown ext', () => {
    const result = processInline('[archive.zip](attachment://zip-001)');
    expect(result).toContain('data-attachment-type="file"');
    expect(result).toContain('data-filename="archive.zip"');
  });
});

describe('processInline — canonical relative Assets/ paths parse correctly', () => {
  it('parses image from root Assets/ path', () => {
    const result = processInline('![photo.jpg](Assets/abc-123.jpg)');
    expect(result).toContain('data-attachment-id="abc-123"');
    expect(result).toContain('data-filename="photo.jpg"');
    expect(result).toContain('<img ');
  });

  it('parses image from ../Assets/ path (one level deep)', () => {
    const result = processInline('![shot.png](../Assets/def-456.png)');
    expect(result).toContain('data-attachment-id="def-456"');
    expect(result).toContain('<img ');
  });

  it('parses image from ../../Assets/ path (two levels deep)', () => {
    const result = processInline('![clip.gif](../../Assets/xyz-999.gif)');
    expect(result).toContain('data-attachment-id="xyz-999"');
    expect(result).toContain('<img ');
  });

  it('parses audio from Assets/ path — stamps data-attachment-type and data-filename', () => {
    const result = processInline('[track.mp3](Assets/aud-001.mp3)');
    expect(result).toContain('data-attachment-id="aud-001"');
    expect(result).toContain('data-attachment-type="audio"');
    expect(result).toContain('data-filename="track.mp3"');
    expect(result).toContain('data-attachment=""');
    expect(result).toContain('<audio ');
  });

  it('parses audio from ../Assets/ path', () => {
    const result = processInline('[track.mp3](../Assets/aud-002.mp3)');
    expect(result).toContain('data-attachment-id="aud-002"');
    expect(result).toContain('data-attachment-type="audio"');
    expect(result).toContain('<audio ');
  });

  it('parses file (pdf) from Assets/ path — stamps data-attachment-type="pdf"', () => {
    const result = processInline('[report.pdf](Assets/doc-001.pdf)');
    expect(result).toContain('data-attachment-id="doc-001"');
    expect(result).toContain('data-attachment-type="pdf"');
    expect(result).toContain('data-filename="report.pdf"');
    expect(result).toContain('attachment-filename');
  });

  it('extracts uuid correctly — strips leading ../ segments', () => {
    const result = processInline('![photo.jpg](../../Assets/uuid-abc-def.jpg)');
    expect(result).toContain('data-attachment-id="uuid-abc-def"');
    expect(result).not.toContain('data-attachment-id="../../Assets');
  });

  it('uses alt text as data-filename for images', () => {
    const result = processInline('![my photo.jpg](Assets/abc.jpg)');
    expect(result).toContain('data-filename="my photo.jpg"');
  });

  it('falls back to uuid+ext as data-filename when alt is empty', () => {
    const result = processInline('![](Assets/abc-123.png)');
    expect(result).toContain('data-attachment-id="abc-123"');
    expect(result).toContain('data-filename="abc-123.png"');
  });
});
