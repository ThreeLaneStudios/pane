/**
 * Editor.generate-image-roundtrip.test.ts
 *
 * #3161 PR1 / PR #3162 — Reed audit Finding 1 (FAIL), Matt's ruling.
 *
 * Reed's audit ( 2026-11-03-audit-3162-pr1-image-gen.md ) found that a
 * generate_image embed — `![caption](Assets/<uuid>.<ext>)`, where `caption` is
 * free-text model prose that may contain a literal `.` and never carries a
 * real file extension — loses its extension (or gains a garbage one) the
 * moment the note is edited and autosaved, because the editor's round trip
 * recovered the extension from `data-filename`/`alt` (the caption) instead of
 * from the attachment's own `src` path. Once the extension is lost, the next
 * load's `Assets/<uuid>(\.[^)]+)` regex no longer matches, `data-attachment-id`
 * is dropped, and the embed is permanently orphaned.
 *
 * WHY THIS TEST MOUNTS A REAL TIPTAP EDITOR (not a pure-function chain):
 *   This project has no full Svelte-component mount harness (#1723), and the
 *   htmlToMarkdown/serializeAttachmentWrapper/convertNodeToMarkdown functions
 *   are not exported from Editor.svelte, so they are re-declared here as
 *   VERBATIM copies (kept byte-for-byte in sync with Editor.svelte — see the
 *   comment above each copy) — same approach Reed's audit took ("extracting
 *   the exact markdown↔HTML logic into a standalone script and running it").
 *
 *   Going one step further than a pure-function chain: this test also mounts
 *   a REAL `@tiptap/core` Editor with the project's real Attachment extension
 *   and calls its real `getHTML()` — not a hand-simulated HTML string. This
 *   matters because `editor.getHTML()`'s schema-based output (a CHILDLESS
 *   wrapper <div>, confirmed empirically — see Attachment.ts renderHTML) is a
 *   different DOM shape than the NodeView's live, editing-only `.attachment-
 *   wrapper`-classed DOM with real <img> children. A pure processInline→
 *   convertNodeToMarkdown chain bypasses this gap entirely (it feeds a raw
 *   <img> tag straight into the serializer); only a real getHTML() round trip
 *   exercises the actual childless-wrapper-div shape the live app produces.
 *   vitest's jsdom environment requires a `window.DOMParser` + `window.document`
 *   polyfill for TipTap/ProseMirror to construct/serialize a real doc at all —
 *   see the `beforeAll` below (same pattern as Attachment.factory-integration.test.ts,
 *   plus `window.document`, which getHTML()'s DOMSerializer also needs and which
 *   this project's global window mock omits).
 *
 * Covers:
 *   GI-1  save → load → save: period-containing, extension-free alt survives
 *         BOTH trips as `![alt](Assets/<uuid>.<ext>)`, data-attachment-id intact.
 *   GI-2  same, but verifies the SECOND trip specifically (the orphaning bug
 *         only manifests on the second load, per Reed's audit).
 *   GI-3  control: a plain file-picker image (data-filename = real filename,
 *         no distinct alt) is unaffected by this fix — same output as before.
 *   GI-4  the bracket caption text itself (not just the extension) survives —
 *         data-filename is now always `<uuid>.<ext>` (self-describing, per
 *         Matt's ruling) and must not clobber the visible caption.
 */

import { describe, it, expect, beforeAll, vi } from 'vitest';
import { Editor } from '@tiptap/core';
import { StarterKit } from '@tiptap/starter-kit';
import { Attachment } from '../extensions/Attachment';
import { buildAttachmentRawHtmlBlock } from '../utils/attachmentSerializer';
import { defaultRenderMode } from '../extensions/Attachment';

// ─── Polyfills for jsdom + TipTap/ProseMirror compatibility ────────────────
// Base DOMParser/setTimeout/event-listener polyfills mirror
// Attachment.factory-integration.test.ts. `window.document` is additionally
// required here because this test (unlike that file) calls `editor.getHTML()`,
// whose DOMSerializer falls back to `window.document.implementation` when no
// explicit `document` option is passed — and this project's global
// `vitest-setup.ts` window mock has no `document` property at all.
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
    if (!(window as any).document && typeof document !== 'undefined') {
      (window as any).document = document;
    }
  }
});

vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn().mockResolvedValue(''),
  openAttachmentExternally: vi.fn().mockResolvedValue(undefined),
}));

// ─── Verbatim copies of Editor.svelte helpers (not exported) ──────────────
// Keep byte-for-byte in sync with Editor.svelte. If Editor.svelte's logic
// changes, this file must be updated to match, or it is testing a fiction.

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

/** Mirrors Editor.svelte processInline's canonical-Assets-form <img> branch, post-fix. */
function imgHtmlFromAssetsMarkdown(alt: string, src: string): string {
  const assetsMatch = src.match(/^(?:\.\.\/)*Assets\/([^./]+)(\.[^)]+)$/);
  if (!assetsMatch) throw new Error('not a canonical Assets/ path: ' + src);
  const attachmentId = assetsMatch[1];
  const ext = assetsMatch[2];
  const safeAlt = escapeHtml(alt);
  const safeSrc = escapeHtml(src);
  const safeId = escapeHtml(attachmentId);
  // Post-fix: always self-describing <uuid>.<ext>, never derived from alt.
  const safeFilename = escapeHtml(attachmentId + ext);
  return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeFilename}">`;
}

/** Mirrors Editor.svelte serializeAttachmentWrapper, post-fix (prefix hardcoded to ''). */
function serializeAttachmentWrapper(el: HTMLElement): string {
  const attachmentId =
    el.getAttribute('data-attachment-id') ||
    el.getAttribute('data-id') ||
    el.querySelector('[data-attachment-id]')?.getAttribute('data-attachment-id') ||
    el.querySelector('[data-id]')?.getAttribute('data-id') ||
    null;

  const renderMode = el.getAttribute('data-render-mode') || null;
  const attachmentType = el.getAttribute('data-attachment-type') || null;
  const widthAttr = el.getAttribute('data-width') || null;
  const heightAttr = el.getAttribute('data-height') || null;
  const hasDimensions = !!(widthAttr || heightAttr);

  const isDefaultMode =
    !renderMode ||
    (attachmentType != null && defaultRenderMode(attachmentType) === renderMode);

  const prefix = '';

  if ((!isDefaultMode || hasDimensions) && attachmentId && attachmentType) {
    const img = el.querySelector('img');
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
    const filenameAttr = el.getAttribute('data-filename') || null;
    const sizeAttr = el.getAttribute('data-size') || null;
    return buildAttachmentRawHtmlBlock({
      attachmentId, renderMode, attachmentType,
      filenameAttr, sizeAttr, widthAttr, heightAttr,
      inner,
    });
  }

  if (!attachmentId) return '';

  const img = el.querySelector('img');
  // Post-fix: TipTap's real getHTML() output for the Attachment node is a
  // CHILDLESS wrapper div (no inner <img> — see Attachment.ts renderHTML), so
  // `attachmentType === 'image'` is required alongside `img` to catch the
  // real production shape, not just legacy hand-built fixtures.
  if (img || attachmentType === 'image') {
    const source = img || el;
    const altText = source.getAttribute('alt') || source.getAttribute('data-filename') || 'image';
    const filenameForExt = source.getAttribute('data-filename') || source.getAttribute('alt') || 'image';
    const srcAttr = source.getAttribute('src') || '';
    const ext = extOf(srcAttr) || extOf(filenameForExt);
    const relPath = `${prefix}Assets/${attachmentId}${ext}`;
    return `![${altText}](${relPath})\n\n`;
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

/** Mirrors Editor.svelte convertNodeToMarkdown (trimmed to the paths this test exercises). */
function convertNodeToMarkdown(node: Node): string {
  let result = '';
  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === Node.TEXT_NODE) { result += child.textContent || ''; continue; }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;
    const el = child as HTMLElement;
    const tag = el.tagName.toLowerCase();
    switch (tag) {
      case 'div':
        if (el.hasAttribute('data-component') && !el.hasAttribute('data-attachment') && !el.classList.contains('attachment-wrapper')) {
          result += '\n' + el.outerHTML + '\n\n';
          break;
        }
        if (el.hasAttribute('data-attachment') || el.classList.contains('attachment-wrapper')) {
          result += serializeAttachmentWrapper(el);
          break;
        }
        result += convertNodeToMarkdown(el);
        break;
      case 'p':
        result += (el.textContent || '') + '\n\n';
        break;
      case 'img': {
        const attachmentId = el.getAttribute('data-attachment-id') || el.getAttribute('data-id');
        const alt = el.getAttribute('alt') || 'image';
        if (attachmentId) {
          const filename = el.getAttribute('data-filename') || alt;
          const srcAttr = el.getAttribute('src') || '';
          const ext = extOf(srcAttr) || extOf(filename);
          const relPath = `Assets/${attachmentId}${ext}`;
          result += `![${alt}](${relPath})\n\n`;
        } else {
          const src = el.getAttribute('src') || '';
          result += `![${alt}](${src})\n\n`;
        }
        break;
      }
      default:
        result += convertNodeToMarkdown(el);
    }
  }
  return result;
}

// ─── Test harness: real TipTap editor, one save trip ───────────────────────

function runOneSaveTrip(imgHtml: string): string {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const editor = new Editor({
    element: el,
    extensions: [StarterKit, Attachment.configure({})],
    content: `<p></p>${imgHtml}`,
  });
  const htmlOut = editor.getHTML();
  const div = document.createElement('div');
  div.innerHTML = htmlOut;
  const markdownOut = convertNodeToMarkdown(div).trim();
  editor.destroy();
  return markdownOut;
}

/** Parses a single `![alt](src)` or `[alt](src)` markdown line back into parts. */
function parseEmbedLine(line: string): { alt: string; src: string; isImage: boolean } {
  const m = line.match(/^(!)?\[([^\]]*)\]\(([^)]+)\)$/);
  if (!m) throw new Error('not a single image/link markdown line: ' + JSON.stringify(line));
  return { isImage: !!m[1], alt: m[2], src: m[3] };
}

describe('GI — generate_image embed survives save → load → save (#3161 PR1, Reed audit Finding 1)', () => {
  const alt = 'A photo of Mr. Smith at dusk'; // period in alt, no extension — the exact garbage-ext scenario
  const attachmentId = '4f9c1a2e-9c3b-4a21-8e77-abcdef123456';
  const src = `Assets/${attachmentId}.png`;

  it('GI-1: trip #1 preserves data-attachment-id and the real extension', () => {
    const html1 = imgHtmlFromAssetsMarkdown(alt, src);
    const md1 = runOneSaveTrip(html1);
    const parsed1 = parseEmbedLine(md1);
    expect(parsed1.isImage).toBe(true);
    expect(parsed1.src).toBe(`Assets/${attachmentId}.png`);
  });

  it('GI-2: trip #2 (the orphaning bug only manifests on the SECOND load) still preserves id + ext', () => {
    const html1 = imgHtmlFromAssetsMarkdown(alt, src);
    const md1 = runOneSaveTrip(html1);
    const parsed1 = parseEmbedLine(md1);

    // Feed trip #1's OWN output back in as if the note were reloaded and saved again.
    const html2 = imgHtmlFromAssetsMarkdown(parsed1.alt, parsed1.src);
    const md2 = runOneSaveTrip(html2);
    const parsed2 = parseEmbedLine(md2);

    expect(parsed2.isImage).toBe(true);
    expect(parsed2.src).toBe(`Assets/${attachmentId}.png`);
    // The regex that reconstructs data-attachment-id on load requires a
    // trailing extension to match at all (Reed's audit, Finding 1) — this is
    // the literal assertion that guards against the orphaning bug.
    expect(parsed2.src).toMatch(/^Assets\/[^./]+\.[^)]+$/);
  });

  it('GI-3: control — a plain file-picker image (filename=photo.jpg, no distinct alt) is unaffected', () => {
    const fileAttachmentId = 'aaa-bbb-ccc';
    const fileSrc = `Assets/${fileAttachmentId}.jpg`;
    // File-picker images set data-filename to the real uploaded filename and never
    // set a distinct `alt` — processInline's canonical-form branch now always sets
    // data-filename to <uuid>.<ext> on LOAD, but a FRESHLY-INSERTED (never-saved)
    // file-picker attachment goes through insertAttachment directly, not processInline;
    // simulate that shape here (filename=photo.jpg, alt unset).
    const imgHtml = `<img src="${fileSrc}" alt="" data-attachment="" data-attachment-id="${fileAttachmentId}" data-attachment-type="image" data-filename="photo.jpg">`;
    const md = runOneSaveTrip(imgHtml);
    const parsed = parseEmbedLine(md);
    expect(parsed.isImage).toBe(true);
    expect(parsed.src).toBe(fileSrc);
    expect(parsed.alt).toBe('photo.jpg');
  });

  it('GI-4: the human caption text survives in the bracket text, not clobbered by the now-self-describing data-filename', () => {
    const html1 = imgHtmlFromAssetsMarkdown(alt, src);
    const md1 = runOneSaveTrip(html1);
    const parsed1 = parseEmbedLine(md1);
    expect(parsed1.isImage).toBe(true);
    expect(parsed1.alt).toBe(alt);
  });
});
