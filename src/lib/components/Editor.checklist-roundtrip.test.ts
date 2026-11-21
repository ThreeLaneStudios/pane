/**
 * Checklist block round-trip regression tests — #2111
 *
 * Verifies that a checklist block (and any generic data-component block) survives
 * the full markdown round-trip:
 *
 *   HTML → convertNodeToMarkdown → markdown string → markdownToHtml → HTML
 *
 * These tests inline the exact logic from Editor.svelte so they run in JSDOM
 * without mounting the Svelte component (same pattern as Editor.attachment-roundtrip.test.ts).
 *
 * Coverage:
 *   CL-1  data-component div serializes as raw-HTML block (not empty string)
 *   CL-2  data-component div outerHTML is preserved verbatim in markdown
 *   CL-3  markdownToHtml passes raw <div data-component="..."> block through intact (C5 guard)
 *   CL-4  Full round-trip: HTML → markdown → HTML contains data-component="checklist"
 *   CL-5  Attachment div (data-attachment) is NOT captured by the data-component branch
 *   CL-6  Checklist block with surrounding text round-trips correctly
 *   CL-7  Generic data-component="due-today" block also round-trips (generic coverage)
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';

// ---------------------------------------------------------------------------
// DOM setup
// ---------------------------------------------------------------------------

let document: Document;

beforeAll(() => {
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  document = dom.window.document;
});

// ---------------------------------------------------------------------------
// Inlined convertNodeToMarkdown — div case (mirrors Editor.svelte post-fix)
// ---------------------------------------------------------------------------

/**
 * Stripped-down convertNodeToMarkdown that only handles the cases relevant to
 * these round-trip tests: text nodes, paragraphs, headings, and divs.
 * The div branch mirrors the post-fix logic in Editor.svelte exactly.
 */
function convertNodeToMarkdown(el: Element): string {
  let result = '';
  for (const child of Array.from(el.childNodes)) {
    if (child.nodeType === 3 /* TEXT_NODE */) {
      result += child.textContent || '';
      continue;
    }
    if (child.nodeType !== 1 /* ELEMENT_NODE */) continue;
    const node = child as Element;
    const tag = node.tagName.toLowerCase();

    switch (tag) {
      case 'p':
        result += (node.textContent || '') + '\n\n';
        break;
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': {
        const level = parseInt(tag.slice(1));
        result += '#'.repeat(level) + ' ' + (node.textContent || '') + '\n\n';
        break;
      }
      case 'div': {
        // Generic data-component block serializer (fixes #2111).
        // Mirrors the post-fix Editor.svelte case 'div': branch exactly.
        if (
          node.hasAttribute('data-component') &&
          !node.hasAttribute('data-attachment') &&
          !node.classList.contains('attachment-wrapper')
        ) {
          result += '\n' + node.outerHTML + '\n\n';
          break;
        }
        // Attachment wrapper (unchanged, pre-existing branch).
        if (node.hasAttribute('data-attachment') || node.classList.contains('attachment-wrapper')) {
          // In real Editor.svelte this calls serializeAttachmentWrapper().
          // For these tests we just emit a stub to confirm the path is taken.
          result += `[attachment:${node.getAttribute('data-attachment-id') ?? 'unknown'}]\n\n`;
          break;
        }
        // Non-special div: recurse
        result += convertNodeToMarkdown(node);
        break;
      }
      default:
        result += convertNodeToMarkdown(node);
    }
  }
  return result;
}

// ---------------------------------------------------------------------------
// Inlined markdownToHtml — block splitter + C5 guard (mirrors Editor.svelte)
// ---------------------------------------------------------------------------

function processInline(text: string): string {
  // Bold
  text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  // Italic
  text = text.replace(/\*(.+?)\*/g, '<em>$1</em>');
  // Inline code
  text = text.replace(/`(.+?)`/g, '<code>$1</code>');
  return text;
}

/**
 * Mirrors markdownToHtml() from Editor.svelte (post-C5 fix).
 * The key behaviour under test: a block that starts with <div is returned
 * verbatim (no <p> wrapping) due to the C5 isBlockElement guard.
 */
function markdownToHtml(md: string): string {
  let html = md;

  // Code blocks first
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_: string, lang: string, code: string) => {
    return `<pre><code class="language-${lang}">${code.trimEnd()}</code></pre>`;
  });

  const blocks: string[] = [];
  let current = '';
  let inCodeBlock = false;

  for (const line of html.split('\n')) {
    if (line.startsWith('<pre>')) {
      if (current.trim()) blocks.push(current.trim());
      current = '';
      inCodeBlock = true;
      current += line + '\n';
      continue;
    }
    if (line.includes('</pre>')) {
      current += line + '\n';
      blocks.push(current.trim());
      current = '';
      inCodeBlock = false;
      continue;
    }
    if (inCodeBlock) {
      current += line + '\n';
      continue;
    }
    if (line.trim() === '') {
      if (current.trim()) blocks.push(current.trim());
      current = '';
    } else {
      current += line + '\n';
    }
  }
  if (current.trim()) blocks.push(current.trim());

  html = blocks.map((block: string) => {
    if (block.startsWith('<pre>')) return block;

    // Headers
    if (block.match(/^#{1,6} /)) {
      const level = block.match(/^(#{1,6}) /)![1].length;
      const text = block.replace(/^#{1,6} /, '');
      return `<h${level}>${processInline(text.trim())}</h${level}>`;
    }

    // Paragraph — with C5 block-element guard
    const processed = processInline(block.trim());
    if (!processed) return '';
    // C5: never wrap a block-level element in <p>
    const isBlockElement = /^<(div|blockquote|pre|ul|ol|table)\b/i.test(processed.trimStart());
    return isBlockElement ? processed : `<p>${processed}</p>`;
  }).join('');

  return html;
}

// ---------------------------------------------------------------------------
// Helper: parse an HTML string into a DOM element
// ---------------------------------------------------------------------------

function parseHTML(html: string): Element {
  const wrapper = document.createElement('div');
  wrapper.innerHTML = html;
  return wrapper.firstElementChild as Element;
}

// ---------------------------------------------------------------------------
// CL-1: data-component div serializes as raw-HTML block (not empty string)
// ---------------------------------------------------------------------------

describe('CL-1 — checklist div serializes to non-empty string', () => {
  it('produces non-empty output for <div data-component="checklist">', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="checklist"></div>';
    const md = convertNodeToMarkdown(wrapper);
    expect(md.trim()).not.toBe('');
  });
});

// ---------------------------------------------------------------------------
// CL-2: data-component div outerHTML is preserved verbatim in markdown
// ---------------------------------------------------------------------------

describe('CL-2 — outerHTML preserved verbatim in serialized markdown', () => {
  it('markdown contains data-component="checklist"', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="checklist"></div>';
    const md = convertNodeToMarkdown(wrapper);
    expect(md).toContain('data-component="checklist"');
  });

  it('markdown contains the full <div> open tag', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="checklist"></div>';
    const md = convertNodeToMarkdown(wrapper);
    expect(md).toContain('<div data-component="checklist">');
  });
});

// ---------------------------------------------------------------------------
// CL-3: markdownToHtml passes raw <div data-component> block through intact
// ---------------------------------------------------------------------------

describe('CL-3 — markdownToHtml C5 guard passes <div data-component> through without <p>', () => {
  it('does NOT wrap the div in <p>', () => {
    const md = '\n<div data-component="checklist"></div>\n\n';
    const html = markdownToHtml(md);
    expect(html).not.toContain('<p><div');
    expect(html).not.toContain('</div></p>');
  });

  it('output contains the data-component div verbatim', () => {
    const md = '\n<div data-component="checklist"></div>\n\n';
    const html = markdownToHtml(md);
    expect(html).toContain('<div data-component="checklist">');
  });
});

// ---------------------------------------------------------------------------
// CL-4: Full round-trip
// ---------------------------------------------------------------------------

describe('CL-4 — full round-trip: HTML → markdown → HTML', () => {
  it('data-component="checklist" survives the full round-trip', () => {
    // Step 1: start with the HTML that TipTap's getHTML() produces for a checklist node
    // (Checklist.ts renderHTML emits exactly this sentinel)
    const inputHtml = '<div data-component="checklist"></div>';
    const wrapper = document.createElement('div');
    wrapper.innerHTML = inputHtml;

    // Step 2: serialize to markdown (what convertNodeToMarkdown does during save)
    const md = convertNodeToMarkdown(wrapper);

    // Step 3: parse markdown back to HTML (what markdownToHtml does on load)
    const outputHtml = markdownToHtml(md);

    // Step 4: verify the sentinel survived
    expect(outputHtml).toContain('data-component="checklist"');
  });

  it('surrounding paragraph text also survives round-trip', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<p>Above the checklist</p><div data-component="checklist"></div><p>Below the checklist</p>';

    const md = convertNodeToMarkdown(wrapper);
    const outputHtml = markdownToHtml(md);

    expect(outputHtml).toContain('Above the checklist');
    expect(outputHtml).toContain('data-component="checklist"');
    expect(outputHtml).toContain('Below the checklist');
  });
});

// ---------------------------------------------------------------------------
// CL-5: Attachment div (data-attachment) NOT captured by data-component branch
// ---------------------------------------------------------------------------

describe('CL-5 — attachment div is NOT captured by data-component branch', () => {
  it('div with only data-attachment routes to attachment branch (not data-component)', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div class="attachment-wrapper" data-attachment="" data-attachment-id="abc123" data-filename="test.mp3"></div>';
    const md = convertNodeToMarkdown(wrapper);
    // Should route to attachment serializer stub, not outerHTML emission
    expect(md).toContain('attachment:abc123');
    expect(md).not.toContain('data-attachment-wrapper');
  });

  it('div with data-component AND data-attachment routes to attachment branch', () => {
    // Edge case: if a future div carries both sentinels, attachment wins
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="checklist" data-attachment="" data-attachment-id="xyz789"></div>';
    const md = convertNodeToMarkdown(wrapper);
    // Should route to attachment branch (data-attachment present)
    expect(md).toContain('attachment:xyz789');
  });
});

// ---------------------------------------------------------------------------
// CL-6: Checklist block with surrounding text round-trips correctly
// ---------------------------------------------------------------------------

describe('CL-6 — checklist block between paragraphs', () => {
  it('text above is not dropped', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<p>Note header</p><div data-component="checklist"></div>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('Note header');
    expect(html).toContain('data-component="checklist"');
  });

  it('text below is not dropped', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="checklist"></div><p>Footer text</p>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('data-component="checklist"');
    expect(html).toContain('Footer text');
  });
});

// ---------------------------------------------------------------------------
// CL-7: Generic data-component="due-today" round-trips (generic coverage)
// ---------------------------------------------------------------------------

describe('CL-7 — generic data-component blocks also round-trip (due-today, etc.)', () => {
  it('data-component="due-today" survives round-trip', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="due-today"></div>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('data-component="due-today"');
  });

  it('data-component="custom-block" survives round-trip', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="custom-block" data-id="42"></div>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('data-component="custom-block"');
  });
});

// ---------------------------------------------------------------------------
// CL-8: DueToday block with data-date attribute round-trips with date intact
// ---------------------------------------------------------------------------

describe('CL-8 — dueToday block data-date attribute survives round-trip', () => {
  it('data-date="2026-07-20" is preserved in serialized markdown', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="dueToday" data-date="2026-07-20"></div>';
    const md = convertNodeToMarkdown(wrapper);
    expect(md).toContain('data-date="2026-07-20"');
  });

  it('data-date="2026-07-20" survives full round-trip HTML → markdown → HTML', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="dueToday" data-date="2026-07-20"></div>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('data-component="dueToday"');
    expect(html).toContain('data-date="2026-07-20"');
  });

  it('block with surrounding text round-trips with date intact', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML =
      '<p>Above</p><div data-component="dueToday" data-date="2026-07-20"></div><p>Below</p>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).toContain('data-component="dueToday"');
    expect(html).toContain('data-date="2026-07-20"');
    expect(html).toContain('Above');
    expect(html).toContain('Below');
  });

  it('does NOT wrap the dueToday div in <p>', () => {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<div data-component="dueToday" data-date="2026-07-20"></div>';
    const md = convertNodeToMarkdown(wrapper);
    const html = markdownToHtml(md);
    expect(html).not.toContain('<p><div');
    expect(html).not.toContain('</div></p>');
  });
});
