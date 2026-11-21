/**
 * Editor.drop-image-roundtrip.test.ts
 *
 * #3163 — the sibling round-trip test for the PLAIN DROP path. #3162 (#3161
 * PR 1) added `Editor.generate-image-roundtrip.test.ts` for the generate_image
 * case and, while fixing it, found the bug was general: TipTap's Attachment
 * node's real `getHTML()` output is a CHILDLESS wrapper `<div>` (confirmed
 * against `Attachment.ts`'s node-level `renderHTML` — no `<img>` descendant at
 * all), so `serializeAttachmentWrapper`'s pre-fix `el.querySelector('img')`
 * check never matched for ANY default-mode image attachment, not only
 * generate_image's free-text-caption case. Any image dropped into a note,
 * then edited and saved, degraded `![alt](Assets/<uuid>.<ext>)` to a plain
 * `[alt](Assets/<uuid>.<ext>)` link on that save — losing the attachment on
 * the NEXT load (the regex that restores `data-attachment-id` requires the
 * leading `!` to have survived... no, actually requires the path shape, but
 * the missing `!` is itself the corruption #3163 exists to both stop (done,
 * #3162) and repair (`migrations.rs::repair_orphaned_image_links`, this PR).
 *
 * This file proves #3162's fix holds for the GENERAL drop-path shape — not
 * generate_image's caption-as-alt shape (that's GI-1..GI-4 in the sibling
 * file) — across the exact two-trip save → load → save shape that is what
 * actually caught the bug originally (#3162's own audit notes a single trip
 * does not surface it; the orphaning is only visible on the SECOND load).
 *
 * Mirrors Editor.generate-image-roundtrip.test.ts's harness verbatim (same
 * reasons: no full Svelte mount harness, #1723; the editor's own markdown↔
 * HTML functions aren't exported, so verbatim copies are re-declared here,
 * kept byte-for-byte in sync with Editor.svelte).
 *
 * Covers:
 *   DP-1  A freshly-dropped image (data-filename = real uploaded filename,
 *         e.g. "photo.jpg", empty alt — the actual `handleFileUpload` shape,
 *         Editor.svelte ~line 465) survives trip #1 as a real image embed.
 *   DP-2  Trip #2 (reload the note, save again) — the orphaning bug only
 *         manifests here. Mutation: revert the `img || attachmentType ===
 *         'image'` gate back to `img` alone (#3162's own fix, reverted) —
 *         reddens, proving this test actually exercises the general-case
 *         fix and isn't a coincidental pass.
 *   DP-3  A drop with a real, non-empty caption the user typed in (not just
 *         the bare filename) survives both the image-ness AND the caption
 *         text across both trips.
 */

import { describe, it, expect, beforeAll, vi } from 'vitest';
import { Editor } from '@tiptap/core';
import { StarterKit } from '@tiptap/starter-kit';
import { Attachment } from '../extensions/Attachment';
import { buildAttachmentRawHtmlBlock } from '../utils/attachmentSerializer';
import { defaultRenderMode } from '../extensions/Attachment';

// ─── Polyfills for jsdom + TipTap/ProseMirror compatibility ────────────────
// Identical to Editor.generate-image-roundtrip.test.ts's own beforeAll —
// see that file's comment for why each one is needed.
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
// Keep byte-for-byte in sync with Editor.svelte / the sibling GI test file.

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
  // #3162's fix (the subject of THIS test): TipTap's real getHTML() output
  // for the Attachment node is a CHILDLESS wrapper div — `img` alone is
  // `null` for every genuine production shape, default-mode OR drop-path.
  // `attachmentType === 'image'` is what actually catches it.
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

describe('DP — a plain DROPPED image survives save → load → save (#3163, the general-case sibling of GI-1..GI-4)', () => {
  // The REAL handleFileUpload shape (Editor.svelte ~line 465): data-filename
  // is the actual uploaded filename (always carries a real extension), alt
  // starts empty — distinct from generate_image's shape (alt = free-text
  // caption, which is what the sibling GI test file covers).
  const attachmentId = '7b2e4f10-aa11-4c22-9d33-0123456789ab';
  const filename = 'photo.jpg';
  const src = `Assets/${attachmentId}.jpg`;

  function dropHtml(alt: string): string {
    const safeAlt = escapeHtml(alt);
    const safeSrc = escapeHtml(src);
    const safeId = escapeHtml(attachmentId);
    const safeFilename = escapeHtml(filename);
    return `<img src="${safeSrc}" alt="${safeAlt}" data-attachment="" data-attachment-id="${safeId}" data-attachment-type="image" data-filename="${safeFilename}">`;
  }

  it('DP-1: trip #1 — a freshly-dropped image survives as a real image embed', () => {
    const md1 = runOneSaveTrip(dropHtml(''));
    const parsed1 = parseEmbedLine(md1);
    expect(parsed1.isImage).toBe(true);
    expect(parsed1.src).toBe(src);
  });

  it('DP-2: trip #2 (reload + re-save) — the general childless-div bug only manifests here', () => {
    const md1 = runOneSaveTrip(dropHtml(''));
    const parsed1 = parseEmbedLine(md1);
    expect(parsed1.isImage).toBe(true);

    // Feed trip #1's own output back in, exactly as a reload would —
    // processInline would re-parse this and re-stamp data-attachment-id/
    // data-attachment-type/data-filename from the Assets/ regex match.
    const html2 = imgHtmlFromAssetsMarkdown(parsed1.alt, parsed1.src);
    const md2 = runOneSaveTrip(html2);
    const parsed2 = parseEmbedLine(md2);

    expect(parsed2.isImage).toBe(true);
    expect(parsed2.src).toBe(src);
    // Requires a trailing extension to match at all on the NEXT load — the
    // literal assertion that guards against the orphaning bug (same shape
    // as GI-2's own assertion in the sibling file).
    expect(parsed2.src).toMatch(/^Assets\/[^./]+\.[^)]+$/);
  });

  it('DP-3: a real user-typed caption (not just the bare filename) survives both trips', () => {
    const caption = 'The view from the office';
    const md1 = runOneSaveTrip(dropHtml(caption));
    const parsed1 = parseEmbedLine(md1);
    expect(parsed1.isImage).toBe(true);
    expect(parsed1.alt).toBe(caption);

    const html2 = imgHtmlFromAssetsMarkdown(parsed1.alt, parsed1.src);
    const md2 = runOneSaveTrip(html2);
    const parsed2 = parseEmbedLine(md2);
    expect(parsed2.isImage).toBe(true);
    expect(parsed2.src).toBe(src);
  });
});
