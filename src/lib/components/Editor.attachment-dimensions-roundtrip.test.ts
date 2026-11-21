/**
 * Editor.attachment-dimensions-roundtrip.test.ts — PR4 (#401)
 *
 * Round-trip guard for width/height dimension attrs.
 * Follows the #1839 sentinel test pattern (Editor.attachment-data-sentinel.test.ts).
 *
 * ROOT CAUSE THIS GUARDS AGAINST:
 *   PR4 adds data-width / data-height attrs to the Attachment node schema.
 *   The serializer (serializeAttachmentWrapper in Editor.svelte) must emit
 *   these attrs in the raw-HTML block so they survive:
 *     getHTML() → serializeAttachmentWrapper → markdown → parseHTML → node attrs
 *   Failure = dimensions lost on save → image snaps back to natural size on reload.
 *
 * This is the same class of bug as #1838 (data-attachment sentinel) and #1839 (data-render-mode).
 * Tests mirror the S-1 through S-8 pattern of Editor.attachment-data-sentinel.test.ts.
 *
 * Tests:
 *  RT-1  Node with width/height → getHTML() → data-width + data-height in emitted div
 *  RT-2  HTML <div data-attachment data-width="640" data-height="480"> → parseHTML → attrs = { width: 640, height: 480 }
 *  RT-3  Full round-trip: schema-serialize → serializeAttachmentWrapper → markdown contains dims → re-parse → attrs preserved
 *  RT-4  Node with null dims → round-trip → attrs stay null (not "null" string, not 0)
 *  RT-5  Explicit regression guard (named test)
 *  RT-6  Legacy nodes without width/height → still round-trip cleanly
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';

// ---------------------------------------------------------------------------
// Inline mirrors of the relevant Editor.svelte helpers
// Mirror the exact production logic so we test the same code paths.
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

/**
 * Mirror of serializeAttachmentWrapper from Editor.svelte (PR4 version).
 *
 * KEY CHANGE vs PR3: hasDimensions path forces raw-HTML block even when
 * renderMode is the default (so data-width/data-height survive the round-trip).
 */
function serializeAttachmentWrapper(el: HTMLElement): string {
  const attachmentId =
    el.getAttribute('data-attachment-id') ||
    el.getAttribute('data-id') ||
    el.querySelector('[data-attachment-id]')?.getAttribute('data-attachment-id') ||
    el.querySelector('[data-id]')?.getAttribute('data-id') ||
    null;

  const renderMode = el.getAttribute('data-render-mode') || null;
  const attachmentType = el.getAttribute('data-attachment-type') || null;

  // PR4 additions
  const widthAttr = el.getAttribute('data-width') || null;
  const heightAttr = el.getAttribute('data-height') || null;
  const hasDimensions = !!(widthAttr || heightAttr);

  const isDefaultMode =
    !renderMode ||
    (attachmentType != null && ATTACHMENT_DEFAULT_MODES[attachmentType] === renderMode);

  // PR4 burn area: hasDimensions forces raw-HTML block so dims survive.
  if ((!isDefaultMode || hasDimensions) && attachmentId && attachmentType) {
    const img = el.querySelector('img');
    let filenameForExt = '';
    if (img) {
      filenameForExt = img.getAttribute('data-filename') || img.getAttribute('alt') || '';
    } else {
      filenameForExt =
        el.querySelector('.attachment-filename')?.textContent ||
        el.getAttribute('data-filename') || '';
    }
    const relPath = `Assets/${attachmentId}${extOf(filenameForExt)}`;
    let inner = el.innerHTML;
    inner = inner.replace(new RegExp(`attachment://${attachmentId}`, 'g'), relPath);

    let wrapperOpen = `<div data-attachment data-attachment-id="${escapeHtml(attachmentId)}"`;
    if (renderMode)     wrapperOpen += ` data-render-mode="${escapeHtml(renderMode)}"`;
    if (attachmentType) wrapperOpen += ` data-attachment-type="${escapeHtml(attachmentType)}"`;
    if (widthAttr)      wrapperOpen += ` data-width="${escapeHtml(widthAttr)}"`;
    if (heightAttr)     wrapperOpen += ` data-height="${escapeHtml(heightAttr)}"`;
    wrapperOpen += `>`;
    return `${wrapperOpen}${inner}</div>\n\n`;
  }

  if (!attachmentId) return '';

  const img = el.querySelector('img');
  if (img) {
    const filename = img.getAttribute('data-filename') || img.getAttribute('alt') || 'image';
    const relPath = `Assets/${attachmentId}${extOf(filename)}`;
    return `![${filename}](${relPath})\n\n`;
  }

  const filename =
    el.querySelector('.attachment-filename')?.textContent ||
    el.getAttribute('data-filename') || 'file';
  const relPath = `Assets/${attachmentId}${extOf(filename)}`;
  return `[${filename}](${relPath})\n\n`;
}

/**
 * Mirrors the Attachment.ts renderHTML output for a node with given attrs.
 * Returns the HTML string as TipTap's getHTML() would emit it.
 */
function mockGetHTML(attrs: {
  attachmentId: string;
  attachmentType: string;
  filename?: string;
  renderMode?: string | null;
  width?: number | null;
  height?: number | null;
  duration?: number | null;
  pageCount?: number | null;
}): string {
  const {
    attachmentId,
    attachmentType,
    filename = 'photo.jpg',
    renderMode = null,
    width = null,
    height = null,
    duration = null,
    pageCount = null,
  } = attrs;

  let html = `<div data-attachment=""`;
  html += ` data-attachment-id="${escapeHtml(attachmentId)}"`;
  html += ` data-attachment-type="${escapeHtml(attachmentType)}"`;
  if (filename)    html += ` data-filename="${escapeHtml(filename)}"`;
  if (renderMode)  html += ` data-render-mode="${escapeHtml(renderMode)}"`;
  if (duration !== null && duration !== undefined)
                   html += ` data-duration="${duration}"`;
  if (pageCount !== null && pageCount !== undefined)
                   html += ` data-page-count="${pageCount}"`;
  if (width !== null && width !== undefined)
                   html += ` data-width="${width}"`;
  if (height !== null && height !== undefined)
                   html += ` data-height="${height}"`;
  html += `></div>`;
  return html;
}

/**
 * Mirrors Attachment.ts parseHTML for a single <div data-attachment> element.
 * Returns the parsed node attrs object.
 */
function mockParseHTML(htmlString: string): Record<string, any> {
  const dom = new JSDOM(`<!DOCTYPE html><html><body>${htmlString}</body></html>`);
  const el = dom.window.document.querySelector('[data-attachment]') as HTMLElement | null;
  if (!el) return {};

  const widthStr = el.getAttribute('data-width');
  const heightStr = el.getAttribute('data-height');
  const durationStr = el.getAttribute('data-duration');
  const pageCountStr = el.getAttribute('data-page-count');

  return {
    attachmentId:   el.getAttribute('data-attachment-id') || null,
    attachmentType: el.getAttribute('data-attachment-type') || null,
    filename:       el.getAttribute('data-filename') || null,
    renderMode:     el.getAttribute('data-render-mode') || null,
    width:          widthStr !== null ? Number(widthStr) : null,
    height:         heightStr !== null ? Number(heightStr) : null,
    duration:       durationStr !== null ? Number(durationStr) : null,
    pageCount:      pageCountStr !== null ? Number(pageCountStr) : null,
  };
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
// RT-1: renderHTML emits data-width + data-height
// ---------------------------------------------------------------------------

describe('RT-1 — node with width/height → getHTML() emits data-width and data-height', () => {
  it('data-width="640" present in getHTML() output', () => {
    const html = mockGetHTML({
      attachmentId: 'img-rt1-001',
      attachmentType: 'image',
      width: 640,
      height: 480,
    });
    expect(html).toContain('data-width="640"');
  });

  it('data-height="480" present in getHTML() output', () => {
    const html = mockGetHTML({
      attachmentId: 'img-rt1-001',
      attachmentType: 'image',
      width: 640,
      height: 480,
    });
    expect(html).toContain('data-height="480"');
  });

  it('both data-width and data-height present on the wrapper div', () => {
    const html = mockGetHTML({
      attachmentId: 'img-rt1-002',
      attachmentType: 'image',
      filename: 'sunset.jpg',
      renderMode: 'full-inline',
      width: 800,
      height: 450,
    });
    expect(html).toContain('data-width="800"');
    expect(html).toContain('data-height="450"');
    expect(html).toContain('data-attachment-id="img-rt1-002"');
  });

  it('null width/height → data-width and data-height NOT emitted (clean HTML)', () => {
    const html = mockGetHTML({
      attachmentId: 'img-rt1-003',
      attachmentType: 'image',
      width: null,
      height: null,
    });
    expect(html).not.toContain('data-width');
    expect(html).not.toContain('data-height');
  });
});

// ---------------------------------------------------------------------------
// RT-2: parseHTML reads data-width / data-height as Numbers
// ---------------------------------------------------------------------------

describe('RT-2 — HTML with data-width/data-height → parseHTML → attrs as numbers', () => {
  it('parses data-width="640" → width: 640 (number)', () => {
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt2-001" data-attachment-type="image" data-width="640" data-height="480"></div>`
    );
    expect(attrs.width).toBe(640);
    expect(typeof attrs.width).toBe('number');
  });

  it('parses data-height="480" → height: 480 (number)', () => {
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt2-001" data-attachment-type="image" data-width="640" data-height="480"></div>`
    );
    expect(attrs.height).toBe(480);
    expect(typeof attrs.height).toBe('number');
  });

  it('absent data-width → width: null (not 0, not "null")', () => {
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt2-002" data-attachment-type="image"></div>`
    );
    expect(attrs.width).toBeNull();
    expect(attrs.width).not.toBe(0);
    expect(attrs.width).not.toBe('null');
  });

  it('absent data-height → height: null (not 0, not "null")', () => {
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt2-002" data-attachment-type="image"></div>`
    );
    expect(attrs.height).toBeNull();
    expect(attrs.height).not.toBe(0);
    expect(attrs.height).not.toBe('null');
  });

  it('integer strings parse to integer numbers', () => {
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt2-003" data-attachment-type="image" data-width="1920" data-height="1080"></div>`
    );
    expect(attrs.width).toBe(1920);
    expect(attrs.height).toBe(1080);
    expect(Number.isInteger(attrs.width)).toBe(true);
    expect(Number.isInteger(attrs.height)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// RT-3: Full round-trip (getHTML → serialize → re-parse → attrs preserved)
// ---------------------------------------------------------------------------

describe('RT-3 — full round-trip: schema-serialize → serializeAttachmentWrapper → re-parse → attrs preserved', () => {
  it('width 640 + height 480 survive the full round-trip', () => {
    // Step 1: schema getHTML() output
    const html = mockGetHTML({
      attachmentId: 'rt3-img-001',
      attachmentType: 'image',
      filename: 'photo.jpg',
      renderMode: 'full-inline',
      width: 640,
      height: 480,
    });

    // Step 2: serializeAttachmentWrapper (Editor.svelte burn area)
    const el = makeEl(html);
    const markdown = serializeAttachmentWrapper(el);

    // Step 3: serialized markdown must contain dimension attrs
    expect(markdown).toContain('data-width="640"');
    expect(markdown).toContain('data-height="480"');
    expect(markdown).toContain('rt3-img-001');

    // Step 4: re-parse the raw-HTML block back to node attrs
    const reParsedAttrs = mockParseHTML(markdown);
    expect(reParsedAttrs.width).toBe(640);
    expect(reParsedAttrs.height).toBe(480);
    expect(typeof reParsedAttrs.width).toBe('number');
    expect(typeof reParsedAttrs.height).toBe('number');
  });

  it('different dimensions (800x600) also survive the round-trip', () => {
    const html = mockGetHTML({
      attachmentId: 'rt3-img-002',
      attachmentType: 'image',
      filename: 'wallpaper.jpg',
      renderMode: 'full-inline',
      width: 800,
      height: 600,
    });

    const el = makeEl(html);
    const markdown = serializeAttachmentWrapper(el);
    const reParsedAttrs = mockParseHTML(markdown);

    expect(reParsedAttrs.width).toBe(800);
    expect(reParsedAttrs.height).toBe(600);
  });

  it('serializeAttachmentWrapper emits raw-HTML block (not plain markdown) when dims are set', () => {
    // Key assertion: dims force the raw-HTML path (not "![photo.jpg](path)")
    const html = mockGetHTML({
      attachmentId: 'rt3-img-003',
      attachmentType: 'image',
      filename: 'thumb.jpg',
      renderMode: 'full-inline',
      width: 320,
      height: 240,
    });

    const el = makeEl(html);
    const markdown = serializeAttachmentWrapper(el);

    // Must NOT produce plain markdown image (which would lose dims)
    expect(markdown).not.toMatch(/^!\[/);
    // Must produce a raw-HTML block
    expect(markdown).toMatch(/<div data-attachment/);
    expect(markdown).toContain('data-width="320"');
    expect(markdown).toContain('data-height="240"');
  });
});

// ---------------------------------------------------------------------------
// RT-4: null dims → round-trip → attrs stay null
// ---------------------------------------------------------------------------

describe('RT-4 — null width/height round-trip → attrs stay null', () => {
  it('image with null dims serializes WITHOUT raw-HTML block (no data-width/height)', () => {
    // An image with no dims and renderMode=full-inline (which IS the default for image)
    // falls through to the default-mode path (no raw-HTML block).
    // The serializer produces a file-relative reference (not a dims-carrying raw block).
    const html = mockGetHTML({
      attachmentId: 'rt4-img-001',
      attachmentType: 'image',
      filename: 'plain.jpg',
      renderMode: 'full-inline',
      width: null,
      height: null,
    });

    const el = makeEl(html);
    const markdown = serializeAttachmentWrapper(el);

    // No dims AND default mode → no raw-HTML block (no data-width/height in output)
    expect(markdown).not.toContain('data-width');
    expect(markdown).not.toContain('data-height');
    // Contains the attachment ID (not dropped)
    expect(markdown).toContain('rt4-img-001');
    // Contains the filename
    expect(markdown).toContain('plain.jpg');
    // Not a raw HTML block
    expect(markdown).not.toMatch(/<div data-attachment/);
  });

  it('plain markdown image re-parsed → width/height remain null', () => {
    // Plain markdown `![plain.jpg](Assets/rt4-img-001.jpg)` doesn't carry dims.
    // A re-parse from raw markdown → the node gets null dims (schema default).
    // We verify parseHTML of a dim-less div → null attrs (not "null" string).
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="rt4-img-002" data-attachment-type="image" data-filename="plain.jpg"></div>`
    );
    expect(attrs.width).toBeNull();
    expect(attrs.height).toBeNull();
    // Not the string "null"
    expect(attrs.width).not.toBe('null');
    expect(attrs.height).not.toBe('null');
    // Not zero
    expect(attrs.width).not.toBe(0);
    expect(attrs.height).not.toBe(0);
  });

  it('null attrs in getHTML output → no data-width / data-height emitted', () => {
    const html = mockGetHTML({
      attachmentId: 'rt4-img-003',
      attachmentType: 'image',
      width: null,
      height: null,
    });
    expect(html).not.toContain('data-width');
    expect(html).not.toContain('data-height');
    expect(html).not.toContain('"null"');
    expect(html).not.toContain('"0"');
  });
});

// ---------------------------------------------------------------------------
// RT-5: Explicit regression guard (Matt's named test)
// ---------------------------------------------------------------------------

describe('RT-5 — regression: attachment with width/height survives edit + re-serialize (guards #1838 for dimensions)', () => {
  it('regression: attachment with width/height survives edit + re-serialize (guards #1838 for dimensions)', () => {
    // This is the exact named test Matt requested.
    // Simulates: user resizes image → editor state updated → note saved →
    //   note reloaded → image should render at the saved size.

    // 1. A resized image node with dims (as getHTML() would emit post-resize)
    const getHTMLOutput = mockGetHTML({
      attachmentId: 'regression-img-001',
      attachmentType: 'image',
      filename: 'resized-photo.jpg',
      renderMode: 'full-inline',
      width: 640,
      height: 480,
    });

    // 2. Assert dims in getHTML output (renderHTML emits them)
    expect(getHTMLOutput).toContain('data-width="640"');
    expect(getHTMLOutput).toContain('data-height="480"');

    // 3. Serialize through Editor.svelte serializer (the burn area)
    const el = makeEl(getHTMLOutput);
    const serialized = serializeAttachmentWrapper(el);

    // 4. Dims must NOT be dropped by the serializer
    expect(serialized).toContain('data-width="640"');
    expect(serialized).toContain('data-height="480"');

    // 5. Re-parse from the serialized markdown block
    const reloaded = mockParseHTML(serialized);

    // 6. Attrs must be Numbers, not strings, not null
    expect(reloaded.width).toBe(640);
    expect(reloaded.height).toBe(480);
    expect(typeof reloaded.width).toBe('number');
    expect(typeof reloaded.height).toBe('number');
    expect(reloaded.width).not.toBe('640');
    expect(reloaded.height).not.toBe('480');
  });
});

// ---------------------------------------------------------------------------
// RT-6: Legacy nodes without width/height → still round-trip cleanly
// ---------------------------------------------------------------------------

describe('RT-6 — legacy nodes without width/height → round-trip cleanly', () => {
  it('legacy image node (no data-width/data-height) serializes as plain markdown', () => {
    const legacyHTML =
      `<div class="attachment-wrapper" data-attachment data-attachment-id="legacy-img-001" ` +
      `data-attachment-type="image" data-filename="old-photo.jpg" data-render-mode="full-inline">` +
      `<img src="Assets/legacy-img-001.jpg" alt="old-photo.jpg" data-filename="old-photo.jpg" data-attachment-id="legacy-img-001">` +
      `</div>`;

    const el = makeEl(legacyHTML);
    const markdown = serializeAttachmentWrapper(el);

    // Legacy node with no dims → plain markdown image
    expect(markdown).toMatch(/!\[old-photo\.jpg\]/);
    expect(markdown).toContain('legacy-img-001');
    // No spurious dim attrs
    expect(markdown).not.toContain('data-width');
    expect(markdown).not.toContain('data-height');
  });

  it('legacy PDF node (no dims) → card link markdown, not dropped', () => {
    const legacyPDF =
      `<div data-attachment data-attachment-id="legacy-pdf-001" data-attachment-type="pdf" data-filename="report.pdf"></div>`;

    const el = makeEl(legacyPDF);
    const markdown = serializeAttachmentWrapper(el);

    expect(markdown).not.toBe('');
    expect(markdown).toContain('legacy-pdf-001');
    expect(markdown).toContain('report.pdf');
  });

  it('legacy audio node → round-trip cleanly (no dims expected)', () => {
    const legacyAudio =
      `<div data-attachment data-attachment-id="legacy-aud-001" data-attachment-type="audio" data-filename="track.mp3"></div>`;

    const el = makeEl(legacyAudio);
    const markdown = serializeAttachmentWrapper(el);

    expect(markdown).not.toBe('');
    expect(markdown).toContain('legacy-aud-001');
    expect(markdown).not.toContain('data-width');
    expect(markdown).not.toContain('data-height');
  });

  it('pre-PR4 image node without dims → parseHTML → width: null, height: null', () => {
    // Nodes created before PR4 have no data-width/data-height.
    // After PR4 schema change, they should parse with null defaults.
    const attrs = mockParseHTML(
      `<div data-attachment data-attachment-id="pre-pr4-img" data-attachment-type="image" data-filename="old.jpg"></div>`
    );
    expect(attrs.width).toBeNull();
    expect(attrs.height).toBeNull();
    // Other attrs still work
    expect(attrs.attachmentId).toBe('pre-pr4-img');
    expect(attrs.attachmentType).toBe('image');
  });
});

// ---------------------------------------------------------------------------
// W-1 Fix Tests (added rev 2)
// ---------------------------------------------------------------------------

import { resolveRenderMode } from '$lib/extensions/Attachment';

// ─── W-1a: resolveRenderMode wiring ──────────────────────────────────────────

describe('W-1a — resolveRenderMode wiring: null renderMode resolves to type default', () => {
  it('image + null renderMode → full-inline (NodeView should pick AttachmentImageInline)', () => {
    // The W-1 regression: after reload from plain-markdown, attrs have
    // attachmentType='image', renderMode=null. NodeView must call resolveRenderMode
    // to get 'full-inline' instead of falling back to card.
    expect(resolveRenderMode('image', null)).toBe('full-inline');
  });

  it('image + explicit card override → card (explicit override wins over type default)', () => {
    // User explicitly set renderMode='card' → must NOT be overridden by default.
    expect(resolveRenderMode('image', 'card')).toBe('card');
  });

  it('image + explicit full-inline → full-inline (explicit valid mode passes through)', () => {
    expect(resolveRenderMode('image', 'full-inline')).toBe('full-inline');
  });

  it('video + null renderMode → card (video default is card, not player)', () => {
    // Ensure resolveRenderMode does not incorrectly apply image defaults to video.
    expect(resolveRenderMode('video', null)).toBe('card');
  });
});

// ─── W-1b: processInline stamps data-attachment-type=image ───────────────────

/**
 * Inline mirror of the processInline image-match section from Editor.svelte
 * as it reads after the W-1 Part 2 fix. Tests must mirror production code exactly
 * (processInline is an internal function and cannot be imported).
 */
function mirrorProcessInlineImages(text: string): string {
  function escapeHtmlInner(s: string): string {
    return s
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }
  return text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (_match, alt, src) => {
    if (src.startsWith('attachment://')) {
      const attachmentId = src.replace('attachment://', '');
      const safeAlt = escapeHtmlInner(alt);
      const safeSrc = escapeHtmlInner(src);
      const safeId = escapeHtmlInner(attachmentId);
      return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeAlt}">`;
    }
    const assetsMatch = src.match(/^(?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)$/);
    if (assetsMatch) {
      const attachmentId = assetsMatch[1];
      const ext = assetsMatch[2];
      const safeAlt = escapeHtmlInner(alt);
      const safeSrc = escapeHtmlInner(src);
      const safeId = escapeHtmlInner(attachmentId);
      const safeFilename = alt ? escapeHtmlInner(alt) : escapeHtmlInner(attachmentId + ext);
      return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeFilename}">`;
    }
    return `<img src="${escapeHtmlInner(src)}" alt="${escapeHtmlInner(alt)}">`;
  });
}

describe('W-1b — processInline stamps data-attachment-type=image on plain markdown <img>', () => {
  it('canonical Assets/ path: emits data-attachment-type="image"', () => {
    const html = mirrorProcessInlineImages('![photo.jpg](Assets/abc-uuid-123.jpg)');
    expect(html).toContain('data-attachment-type="image"');
  });

  it('canonical Assets/ path: emits data-attachment="" sentinel', () => {
    const html = mirrorProcessInlineImages('![photo.jpg](Assets/abc-uuid-123.jpg)');
    expect(html).toContain('data-attachment=""');
  });

  it('canonical Assets/ path: emits data-attachment-id with the UUID', () => {
    const html = mirrorProcessInlineImages('![photo.jpg](Assets/abc-uuid-123.jpg)');
    expect(html).toContain('data-attachment-id="abc-uuid-123"');
  });

  it('legacy attachment:// path: emits all three data attrs', () => {
    const html = mirrorProcessInlineImages('![photo.jpg](attachment://legacy-uuid-001)');
    expect(html).toContain('data-attachment=""');
    expect(html).toContain('data-attachment-id="legacy-uuid-001"');
    expect(html).toContain('data-attachment-type="image"');
  });
});

// ─── W-1c: Non-resized image round-trip: full-inline post-reload ─────────────

describe('W-1c — non-resized image round-trip: full-inline survives save → reload', () => {
  it('processInline output parsed back has attachmentType=image (enables resolveRenderMode to pick full-inline)', () => {
    // Simulate the reload path: markdown → processInline → HTML → TipTap parseHTML.
    // After the fix, the HTML contains data-attachment-type="image".
    const markdown = '![photo.jpg](Assets/img-uuid-999.jpg)';
    const html = mirrorProcessInlineImages(markdown);

    // Parse as TipTap would (via img[src] parseHTML rule).
    const dom = new JSDOM(`<!DOCTYPE html><html><body>${html}</body></html>`);
    const img = dom.window.document.querySelector('img') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    expect(img!.getAttribute('data-attachment-type')).toBe('image');
  });

  it('non-resized image: full W-1 loop — parseHTML gives image+null → resolveRenderMode → full-inline', () => {
    // After processInline stamps data-attachment-type="image":
    //   parseHTML → { attachmentType: 'image', renderMode: null }
    //   NodeView uses resolveRenderMode('image', null) → 'full-inline'
    //   → AttachmentImageInline is rendered, NOT the card fallback.
    const attachmentType = 'image';
    const renderMode = null; // no data-render-mode attr in plain-markdown reload path
    expect(resolveRenderMode(attachmentType, renderMode)).toBe('full-inline');
  });
});
