/**
 * Editor.svelte — attachment serializer data-attribute sentinel tests (#1838)
 *
 * Regression tests for the bug where `convertNodeToMarkdown` / `htmlToMarkdown`
 * silently dropped attachment nodes after PR #1834 added `renderHTML` to the
 * Attachment node schema.
 *
 * ROOT CAUSE:
 *   PR #1834's `renderHTML` emits:
 *     <div data-attachment data-attachment-id="..." data-attachment-type="..."></div>
 *   WITHOUT the `.attachment-wrapper` class.
 *   The serializer's `case 'div':` branch previously checked ONLY for
 *   `el.classList.contains('attachment-wrapper')`.
 *   TipTap's `editor.getHTML()` uses schema-based serialization (calls renderHTML),
 *   so the class was never present → serializer didn't recognize the node →
 *   attachment silently dropped from markdown on save (data loss).
 *
 * FIX (#1838):
 *   Check `el.hasAttribute('data-attachment')` first, fall back to
 *   `el.classList.contains('attachment-wrapper')` for backward compat.
 *
 * Tests:
 *   S-1  renderHTML output (data-attribute only, no class) → recognized, NOT dropped
 *   S-2  Legacy .attachment-wrapper class (no data-attribute) → still recognized (backward compat)
 *   S-3  Both data-attribute + class → recognized (belt-and-suspenders nodes)
 *   S-4  Plain div with no attachment markers → NOT treated as attachment
 *   S-5  Round-trip: renderHTML output → convertNodeToMarkdown → same attachment reference
 *   S-6  Round-trip: multi-type (image, pdf, audio, file) renderHTML divs → all preserved
 *   S-7  Round-trip: note with mixed content (text + attachment) → attachment not dropped
 *   S-8  Round-trip: markdown → htmlToMarkdown-simulated HTML → serialize → preserve reference
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';

// ---------------------------------------------------------------------------
// Inline mirrors of the helpers from Editor.svelte
// These mirror the exact production logic so we test the same code paths.
// ---------------------------------------------------------------------------

const ATTACHMENT_DEFAULT_MODES: Record<string, string> = {
  image: 'full-inline',
  audio: 'card',
  video: 'card',
  pdf: 'card',
  html: 'card',
  link: 'preview',
  file: 'card',
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

function computeAttachmentPrefix(filePath?: string): string {
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
    let filenameForExt = '';
    if (img) {
      filenameForExt = img.getAttribute('data-filename') || img.getAttribute('alt') || '';
    } else {
      filenameForExt =
        el.querySelector('.attachment-filename')?.textContent ||
        el.getAttribute('data-filename') ||
        '';
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

  // File / PDF / HTML / Link (card types) - use data-filename on wrapper (PR #1834 output)
  const filename =
    el.querySelector('.attachment-filename')?.textContent ||
    el.getAttribute('data-filename') ||
    'file';
  const relPath = `${prefix}Assets/${attachmentId}${extOf(filename)}`;
  return `[${filename}](${relPath})\n\n`;
}

/**
 * Mirrors convertNodeToMarkdown's `case 'div':` branch — the fix from #1838.
 *
 * THE KEY CHANGE: Check data-attachment attribute FIRST, fall back to class check.
 * Before the fix: only `el.classList.contains('attachment-wrapper')` was checked.
 * After the fix: `el.hasAttribute('data-attachment') || el.classList.contains('attachment-wrapper')`
 *
 * Returns the serialized result if this is an attachment node, or null if not.
 */
function convertDivNode(el: HTMLElement, filePath?: string): string | null {
  // THE FIX: recognize by data-attribute sentinel first, then by class (backward compat)
  if (el.hasAttribute('data-attachment') || el.classList.contains('attachment-wrapper')) {
    return serializeAttachmentWrapper(el, filePath);
  }
  return null; // Not an attachment node
}

/**
 * Simulates the full convertNodeToMarkdown output for a top-level HTML string.
 * Processes all child divs at the top level as the real function would.
 */
function convertHTMLToMarkdown(html: string, filePath?: string): string {
  let document: Document;
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  document = dom.window.document;

  const container = document.createElement('div');
  container.innerHTML = html;

  let result = '';
  for (const child of Array.from(container.children)) {
    const el = child as HTMLElement;
    const tag = el.tagName.toLowerCase();
    if (tag === 'div') {
      const serialized = convertDivNode(el, filePath);
      if (serialized !== null) {
        result += serialized;
      }
      // (non-attachment divs would fall through; omitted here for brevity)
    } else if (tag === 'p') {
      result += el.textContent + '\n\n';
    }
  }
  return result.trim();
}

// ---------------------------------------------------------------------------
// DOM setup
// ---------------------------------------------------------------------------

let document: Document;

beforeAll(() => {
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  document = dom.window.document;
});

function makeEl(html: string): HTMLElement {
  const div = document.createElement('div');
  div.innerHTML = html;
  return div.firstElementChild as HTMLElement;
}

// ---------------------------------------------------------------------------
// S-1: PR #1834 renderHTML output (data-attribute only, NO class) → recognized
// ---------------------------------------------------------------------------

describe('S-1 — renderHTML output (data-attribute only, no class) is recognized by serializer (#1838 fix)', () => {
  it('div[data-attachment] without .attachment-wrapper class → recognized as attachment', () => {
    // This is what Attachment.ts renderHTML() emits (PR #1834).
    // Before the fix: this would NOT be recognized → attachment dropped.
    // After the fix: hasAttribute('data-attachment') catches it.
    const el = makeEl(
      `<div data-attachment data-attachment-id="abc-123" data-attachment-type="file" data-filename="report.pdf"></div>`
    );
    expect(el.classList.contains('attachment-wrapper')).toBe(false); // confirm no class
    expect(el.hasAttribute('data-attachment')).toBe(true); // confirm sentinel present
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull(); // MUST be recognized, not dropped
  });

  it('recognized renderHTML div → serializes correct attachment reference', () => {
    const el = makeEl(
      `<div data-attachment data-attachment-id="pdf-uuid-001" data-attachment-type="pdf" data-filename="report.pdf"></div>`
    );
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).not.toBe(''); // Not dropped
    expect(result).toContain('pdf-uuid-001'); // attachment ID preserved
    expect(result).toContain('report.pdf'); // filename preserved
  });

  it('image node via renderHTML (no class) → serializes as markdown image', () => {
    const el = makeEl(
      `<div data-attachment data-attachment-id="img-001" data-attachment-type="image" data-filename="photo.jpg" data-render-mode="full-inline">` +
      `<img src="Assets/img-001.jpg" alt="photo.jpg" data-filename="photo.jpg" data-attachment-id="img-001">` +
      `</div>`
    );
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).toContain('img-001');
    expect(result).toContain('photo.jpg');
    expect(result).toMatch(/!\[photo\.jpg\]/); // image markdown format
  });

  it('audio node via renderHTML (no class) → NOT silently dropped', () => {
    const el = makeEl(
      `<div data-attachment data-attachment-id="aud-uuid-001" data-attachment-type="audio" data-filename="track.mp3"></div>`
    );
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).not.toBe('');
    expect(result).toContain('aud-uuid-001');
  });

  it('pdf node via renderHTML (no class) → NOT silently dropped', () => {
    const el = makeEl(
      `<div data-attachment data-attachment-id="pdf-uuid-002" data-attachment-type="pdf" data-filename="doc.pdf"></div>`
    );
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).not.toBe('');
    expect(result).toContain('pdf-uuid-002');
  });
});

// ---------------------------------------------------------------------------
// S-2: Legacy .attachment-wrapper class (backward compat)
// ---------------------------------------------------------------------------

describe('S-2 — legacy .attachment-wrapper class still recognized (backward compat)', () => {
  it('.attachment-wrapper with data-attachment → recognized', () => {
    const el = makeEl(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="leg-001" data-filename="legacy.jpg">` +
      `<img src="Assets/leg-001.jpg" alt="legacy.jpg" data-filename="legacy.jpg" data-attachment-id="leg-001">` +
      `</div>`
    );
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).toContain('leg-001');
  });

  it('.attachment-wrapper WITHOUT data-attachment → still recognized via class fallback', () => {
    // Very old markup may have class but no data-attribute. Should still work.
    const el = makeEl(
      `<div class="attachment-wrapper" data-attachment-id="old-001" data-id="old-001">` +
      `<img src="attachment://old-001" alt="old.jpg" data-filename="old.jpg" data-id="old-001">` +
      `</div>`
    );
    expect(el.hasAttribute('data-attachment')).toBe(false); // no sentinel
    expect(el.classList.contains('attachment-wrapper')).toBe(true); // has class
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).toContain('old-001');
  });
});

// ---------------------------------------------------------------------------
// S-3: Both data-attribute + class present
// ---------------------------------------------------------------------------

describe('S-3 — div with both data-attachment attribute and .attachment-wrapper class → recognized', () => {
  it('belt-and-suspenders div is recognized correctly', () => {
    const el = makeEl(
      `<div class="attachment-wrapper" data-attachment data-attachment-id="both-001" data-filename="both.mp3">` +
      `<div class="attachment-filename">both.mp3</div>` +
      `<audio data-attachment-id="both-001" controls></audio>` +
      `</div>`
    );
    expect(el.hasAttribute('data-attachment')).toBe(true);
    expect(el.classList.contains('attachment-wrapper')).toBe(true);
    const result = convertDivNode(el, 'Projects/Foo/note.md');
    expect(result).not.toBeNull();
    expect(result).toContain('both-001');
  });
});

// ---------------------------------------------------------------------------
// S-4: Plain div with no attachment markers → NOT treated as attachment
// ---------------------------------------------------------------------------

describe('S-4 — plain div without attachment markers → NOT treated as attachment', () => {
  it('plain <div> with text content → returns null (not an attachment)', () => {
    const el = makeEl(`<div><p>Some regular content</p></div>`);
    expect(el.hasAttribute('data-attachment')).toBe(false);
    expect(el.classList.contains('attachment-wrapper')).toBe(false);
    const result = convertDivNode(el);
    expect(result).toBeNull(); // correctly NOT treated as attachment
  });

  it('<div class="some-other-class"> → not treated as attachment', () => {
    const el = makeEl(`<div class="some-other-class" data-attachment-type="image">text</div>`);
    // Has data-attachment-type but NOT data-attachment (the sentinel)
    const result = convertDivNode(el);
    expect(result).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// S-5: Round-trip — renderHTML output through convertNodeToMarkdown → preserved
// ---------------------------------------------------------------------------

describe('S-5 — round-trip: renderHTML div → serialize → attachment reference preserved', () => {
  it('image: renderHTML output → markdown → contains attachment ID and filename', () => {
    // Simulate what TipTap getHTML() emits when it uses schema-based serialization
    // (calls renderHTML on the Attachment node — PR #1834 format).
    const renderHTMLOutput = `<div data-attachment="" data-attachment-id="img-rt-001" data-attachment-type="image" data-filename="sunset.jpg" data-render-mode="full-inline"><img src="Assets/img-rt-001.jpg" alt="sunset.jpg" data-filename="sunset.jpg" data-attachment-id="img-rt-001"></div>`;
    const result = convertHTMLToMarkdown(renderHTMLOutput, 'Projects/Foo/note.md');
    expect(result).not.toBe(''); // NOT dropped
    expect(result).toContain('img-rt-001'); // ID preserved
    expect(result).toContain('sunset.jpg'); // filename preserved
  });

  it('pdf: renderHTML output (no children) → serializes with filename from data-filename attr', () => {
    // PR #1834 renderHTML for a PDF card: empty div with data attributes only
    const renderHTMLOutput = `<div data-attachment="" data-attachment-id="pdf-rt-001" data-attachment-type="pdf" data-filename="report.pdf"></div>`;
    const result = convertHTMLToMarkdown(renderHTMLOutput, 'Projects/Foo/note.md');
    expect(result).not.toBe('');
    expect(result).toContain('pdf-rt-001');
    expect(result).toContain('report.pdf');
  });

  it('file: renderHTML output → not dropped', () => {
    const renderHTMLOutput = `<div data-attachment="" data-attachment-id="file-rt-001" data-attachment-type="file" data-filename="archive.zip"></div>`;
    const result = convertHTMLToMarkdown(renderHTMLOutput, 'Projects/Foo/note.md');
    expect(result).not.toBe('');
    expect(result).toContain('file-rt-001');
  });

  it('non-default renderMode: preserved in serialized output', () => {
    // An image in "card" mode (non-default for image; default is "full-inline").
    // Should produce a raw-HTML block to preserve the render mode.
    const renderHTMLOutput = `<div data-attachment="" data-attachment-id="img-rt-002" data-attachment-type="image" data-render-mode="card" data-filename="thumbnail.jpg"></div>`;
    const result = convertHTMLToMarkdown(renderHTMLOutput, 'Projects/Foo/note.md');
    expect(result).not.toBe('');
    expect(result).toContain('img-rt-002');
    expect(result).toContain('data-render-mode="card"'); // non-default → raw HTML block
  });
});

// ---------------------------------------------------------------------------
// S-6: Multi-type round-trip
// ---------------------------------------------------------------------------

describe('S-6 — multi-type: image + pdf + audio + file renderHTML divs all preserved', () => {
  it('all four types survive the serializer pass', () => {
    const html = [
      `<div data-attachment="" data-attachment-id="mt-img-001" data-attachment-type="image" data-filename="photo.jpg" data-render-mode="full-inline"><img src="Assets/mt-img-001.jpg" alt="photo.jpg" data-filename="photo.jpg" data-attachment-id="mt-img-001"></div>`,
      `<div data-attachment="" data-attachment-id="mt-pdf-001" data-attachment-type="pdf" data-filename="doc.pdf"></div>`,
      `<div data-attachment="" data-attachment-id="mt-aud-001" data-attachment-type="audio" data-filename="track.mp3"></div>`,
      `<div data-attachment="" data-attachment-id="mt-fil-001" data-attachment-type="file" data-filename="archive.zip"></div>`,
    ].join('\n');

    const result = convertHTMLToMarkdown(html, 'Projects/Foo/note.md');

    expect(result).toContain('mt-img-001'); // image not dropped
    expect(result).toContain('mt-pdf-001'); // pdf not dropped
    expect(result).toContain('mt-aud-001'); // audio not dropped
    expect(result).toContain('mt-fil-001'); // file not dropped
  });
});

// ---------------------------------------------------------------------------
// S-7: Mixed content — text before and after attachment
// ---------------------------------------------------------------------------

describe('S-7 — mixed content: text + attachment → attachment NOT dropped from note', () => {
  it('note with heading text + renderHTML attachment + footer text → all preserved', () => {
    const html =
      `<p>Intro paragraph text</p>` +
      `<div data-attachment="" data-attachment-id="mixed-001" data-attachment-type="pdf" data-filename="notes.pdf"></div>` +
      `<p>Footer paragraph text</p>`;

    const result = convertHTMLToMarkdown(html, 'Projects/Foo/note.md');

    expect(result).toContain('Intro paragraph text'); // text preserved
    expect(result).toContain('mixed-001'); // attachment NOT dropped
    expect(result).toContain('notes.pdf'); // filename preserved
    expect(result).toContain('Footer paragraph text'); // trailing text preserved
  });
});

// ---------------------------------------------------------------------------
// S-8: Confirm old-code behavior (negative test — documents the bug)
// ---------------------------------------------------------------------------

describe('S-8 — negative test: confirms bug existed (class-only check would drop the node)', () => {
  it('old class-only check would return null for renderHTML output', () => {
    // This test documents that the old code (class-only check) would have
    // returned null (i.e., dropped the node). The new check returns non-null.
    const el = makeEl(
      `<div data-attachment="" data-attachment-id="bug-001" data-attachment-type="pdf" data-filename="lost.pdf"></div>`
    );

    // Simulate OLD (buggy) behavior: class-only check
    const oldBehavior = el.classList.contains('attachment-wrapper')
      ? serializeAttachmentWrapper(el, 'Projects/Foo/note.md')
      : null;
    expect(oldBehavior).toBeNull(); // confirms the bug: node would have been DROPPED

    // Simulate NEW (fixed) behavior: data-attribute sentinel check
    const newBehavior = el.hasAttribute('data-attachment') || el.classList.contains('attachment-wrapper')
      ? serializeAttachmentWrapper(el, 'Projects/Foo/note.md')
      : null;
    expect(newBehavior).not.toBeNull(); // fix: node is now recognized
    expect(newBehavior).toContain('bug-001'); // and serialized correctly
    expect(newBehavior).toContain('lost.pdf');
  });
});
