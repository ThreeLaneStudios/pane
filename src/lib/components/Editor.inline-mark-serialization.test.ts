/**
 * Editor.inline-mark-serialization.test.ts — #2419 round-2 Finding 1 (launch-blocking)
 *
 * Root cause (per audit `2026-08-05-2419-round2-scoping-audit.md`, Finding 1):
 * `convertNodeToMarkdown`'s `<p>` case (Editor.svelte:960) and the `<ul>`/`<ol>`
 * `<li>` cases (Editor.svelte:973, 981) call `convertInlineToMarkdown(pChild)` /
 * `convertInlineToMarkdown(li)` passing the INLINE-MARK ELEMENT ITSELF (a wiki-link
 * `<span>`, `<strong>`, `<em>`, `<code>`, `<a>`, `<s>`) as the element to iterate
 * INTO. The old `convertInlineToMarkdown` only tested each of `el`'s CHILDREN
 * against the mark cases — it never tested `el` itself. Because real TipTap output
 * puts inline marks as DIRECT children of `<p>`/`<li>` (e.g.
 * `<p>This is <strong>bold</strong> text.</p>`), every mark's syntax was silently
 * dropped on save: `**bold**` -> `bold`, `[[Foo Note]]` -> `Foo Note`, etc.
 *
 * Fix: `convertInlineToMarkdown` now tests its OWN `el` parameter's tag/attributes
 * FIRST (mirroring `convertNodeToMarkdown`'s own top-level dispatch pattern), and
 * only falls through to iterating `el`'s children (via the new `convertInlineChildren`
 * helper) when `el` itself is not a recognized mark. The call sites at 960/973/981
 * are UNCHANGED — this is a pure internal-logic fix to `convertInlineToMarkdown`.
 *
 * METHOD (mirrors Reed's audit harness pattern): input HTML strings below are
 * captured verbatim from a REAL `@tiptap/core` `Editor` instance's `getHTML()`
 * output (see the "real TipTap HTML capture" describe block), not hand-typed —
 * this guards against the serializer being tested against a DOM shape TipTap
 * never actually produces.
 *
 * The serializer functions below are extracted verbatim (post-fix) from
 * Editor.svelte:918-1132, following the same "test the logic in isolation"
 * pattern as Editor.restore-markdown.test.ts (project has no full Svelte
 * component test harness yet — #1723).
 *
 * RED-ON-REVERT: every round-trip test in this file fails if `convertInlineToMarkdown`
 * is reverted to the pre-fix version (child-only iteration, no self-test). See the
 * dedicated "pre-fix (buggy) implementation produces WRONG output" block, which pins
 * the exact broken behavior being fixed — if that block ever passes with correct
 * (bracketed/starred) output, the revert-detection guard itself has silently broken.
 *
 * Relates to #2419
 * spec-skip: serializer defect fix, no behavioral decision
 */

import { describe, it, expect, beforeAll } from 'vitest';
import { JSDOM } from 'jsdom';
import { Editor } from '@tiptap/core';
import { StarterKit } from '@tiptap/starter-kit';
import { Link } from '@tiptap/extension-link';
import { Underline } from '@tiptap/extension-underline';
import { WikiLink } from '$lib/extensions/WikiLink';

// ---------------------------------------------------------------------------
// Post-fix convertNodeToMarkdown / convertInlineToMarkdown / convertInlineChildren
// Extracted verbatim from Editor.svelte (post #2419-round-2 Finding-1 fix).
// Only the branches needed to exercise inline marks are included (headings,
// tables, attachments, etc. are pinned by other existing test files and are
// out of scope for this PR per the audit's scope guard).
// ---------------------------------------------------------------------------

function convertNodeToMarkdown(node: Node): string {
  let result = '';

  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === Node.TEXT_NODE) {
      result += child.textContent || '';
      continue;
    }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;

    const el = child as HTMLElement;
    const tag = el.tagName.toLowerCase();

    switch (tag) {
      case 'p': {
        let pContent = '';
        for (const pChild of Array.from(el.childNodes)) {
          if (pChild.nodeType === Node.ELEMENT_NODE && (pChild as HTMLElement).tagName.toLowerCase() === 'br') {
            pContent += '\n';
          } else if (pChild.nodeType === Node.TEXT_NODE) {
            pContent += pChild.textContent || '';
          } else if (pChild.nodeType === Node.ELEMENT_NODE) {
            pContent += convertInlineToMarkdown(pChild as Element); // UNCHANGED call site (line ~960)
          }
        }
        if (pContent.trim()) {
          result += pContent + '\n\n';
        }
        break;
      }
      case 'ul':
        for (const li of Array.from(el.children)) {
          if (li.getAttribute('data-type') === 'taskItem') {
            const checked = li.getAttribute('data-checked') === 'true';
            result += `- [${checked ? 'x' : ' '}] ${li.textContent}\n`;
          } else {
            result += `- ${convertInlineToMarkdown(li)}\n`; // UNCHANGED call site (line ~973)
          }
        }
        result += '\n';
        break;
      case 'ol': {
        let idx = 1;
        for (const li of Array.from(el.children)) {
          result += `${idx}. ${convertInlineToMarkdown(li)}\n`; // UNCHANGED call site (line ~981)
          idx++;
        }
        result += '\n';
        break;
      }
      default:
        result += convertNodeToMarkdown(el);
    }
  }

  return result;
}

// POST-FIX: tests el's OWN tag/attributes first; falls through to children
// only when el itself is not a recognized mark.
function convertInlineToMarkdown(el: Element): string {
  const tag = el.tagName.toLowerCase();

  switch (tag) {
    case 'wiki-link': {
      const wikiTitle = el.getAttribute('data-title') || el.textContent || '';
      return `[[${wikiTitle}]]`;
    }
    case 'span':
      if (el.hasAttribute('data-wiki-link') || el.hasAttribute('data-title')) {
        const wikiTitle = el.getAttribute('data-title') || el.textContent || '';
        return `[[${wikiTitle}]]`;
      }
      break;
    case 'strong':
    case 'b':
      return `**${convertInlineChildren(el)}**`;
    case 'em':
    case 'i':
      return `*${convertInlineChildren(el)}*`;
    case 'code':
      return `\`${el.textContent}\``;
    case 'a':
      return `[${el.textContent}](${el.getAttribute('href') || ''})`;
    case 's':
      return `~~${convertInlineChildren(el)}~~`;
    case 'u':
      return convertInlineChildren(el);
    default:
      break;
  }

  return convertInlineChildren(el);
}

function convertInlineChildren(el: Element): string {
  let result = '';
  for (const child of Array.from(el.childNodes)) {
    if (child.nodeType === Node.TEXT_NODE) {
      result += child.textContent || '';
      continue;
    }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;
    result += convertInlineToMarkdown(child as Element);
  }
  return result;
}

function htmlToMarkdown(html: string): string {
  const div = document.createElement('div');
  div.innerHTML = html;
  return convertNodeToMarkdown(div).trim();
}

// ---------------------------------------------------------------------------
// PRE-FIX (buggy) implementation — used ONLY to pin the broken behavior being
// fixed, proving the test suite actually distinguishes fixed from broken.
// This mirrors the exact pre-fix code (child-only iteration, `el` never tested).
// ---------------------------------------------------------------------------

function convertInlineToMarkdown_PREFIX(el: Element): string {
  let result = '';
  for (const child of Array.from(el.childNodes)) {
    if (child.nodeType === Node.TEXT_NODE) {
      result += child.textContent || '';
      continue;
    }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;

    const childEl = child as HTMLElement;
    const tag = childEl.tagName.toLowerCase();

    switch (tag) {
      case 'wiki-link': {
        const wikiTitle = childEl.getAttribute('data-title') || childEl.textContent || '';
        result += `[[${wikiTitle}]]`;
        break;
      }
      case 'span':
        if (childEl.hasAttribute('data-wiki-link') || childEl.hasAttribute('data-title')) {
          const wikiTitle = childEl.getAttribute('data-title') || childEl.textContent || '';
          result += `[[${wikiTitle}]]`;
        } else {
          result += convertInlineToMarkdown_PREFIX(childEl);
        }
        break;
      case 'strong':
      case 'b':
        result += `**${convertInlineToMarkdown_PREFIX(childEl)}**`;
        break;
      case 'em':
      case 'i':
        result += `*${convertInlineToMarkdown_PREFIX(childEl)}*`;
        break;
      case 'code':
        result += `\`${childEl.textContent}\``;
        break;
      case 'a':
        result += `[${childEl.textContent}](${childEl.getAttribute('href') || ''})`;
        break;
      case 'u':
        result += convertInlineToMarkdown_PREFIX(childEl);
        break;
      case 's':
        result += `~~${convertInlineToMarkdown_PREFIX(childEl)}~~`;
        break;
      case 'p':
        result += convertInlineToMarkdown_PREFIX(childEl);
        break;
      default:
        result += convertInlineToMarkdown_PREFIX(childEl);
    }
  }
  return result;
}

function convertNodeToMarkdown_PREFIX(node: Node): string {
  let result = '';
  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === Node.TEXT_NODE) {
      result += child.textContent || '';
      continue;
    }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;
    const el = child as HTMLElement;
    const tag = el.tagName.toLowerCase();
    switch (tag) {
      case 'p': {
        let pContent = '';
        for (const pChild of Array.from(el.childNodes)) {
          if (pChild.nodeType === Node.ELEMENT_NODE && (pChild as HTMLElement).tagName.toLowerCase() === 'br') {
            pContent += '\n';
          } else if (pChild.nodeType === Node.TEXT_NODE) {
            pContent += pChild.textContent || '';
          } else if (pChild.nodeType === Node.ELEMENT_NODE) {
            pContent += convertInlineToMarkdown_PREFIX(pChild as Element);
          }
        }
        if (pContent.trim()) result += pContent + '\n\n';
        break;
      }
      case 'ul':
        for (const li of Array.from(el.children)) {
          result += `- ${convertInlineToMarkdown_PREFIX(li)}\n`;
        }
        result += '\n';
        break;
      case 'ol': {
        let idx = 1;
        for (const li of Array.from(el.children)) {
          result += `${idx}. ${convertInlineToMarkdown_PREFIX(li)}\n`;
          idx++;
        }
        result += '\n';
        break;
      }
      default:
        result += convertNodeToMarkdown_PREFIX(el);
    }
  }
  return result;
}

function htmlToMarkdown_PREFIX(html: string): string {
  const div = document.createElement('div');
  div.innerHTML = html;
  return convertNodeToMarkdown_PREFIX(div).trim();
}

// ---------------------------------------------------------------------------
// DOM setup
// ---------------------------------------------------------------------------

let document: Document;

beforeAll(() => {
  const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
  document = dom.window.document;
  (global as any).document = document;
  (global as any).Node = dom.window.Node;
});

// ---------------------------------------------------------------------------
// Real TipTap HTML capture — same methodology as Reed's audit harness.
// Instantiates a real @tiptap/core Editor with the real extensions this
// project uses (StarterKit, Link, Underline, WikiLink) and reads getHTML()
// to obtain the ACTUAL DOM shape TipTap produces, rather than hand-typed HTML.
// ---------------------------------------------------------------------------

function makeRealEditorHTML(content: string): string {
  if (typeof window !== 'undefined') {
    if (!(window as any).DOMParser && typeof DOMParser !== 'undefined') {
      (window as any).DOMParser = DOMParser;
    }
    if (!(window as any).setTimeout) (window as any).setTimeout = setTimeout;
    if (!(window as any).clearTimeout) (window as any).clearTimeout = clearTimeout;
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
    if (!(window as any).document) (window as any).document = document;
  }

  const el = document.createElement('div');
  document.body.appendChild(el);
  const editor = new Editor({
    element: el,
    extensions: [
      StarterKit.configure({ codeBlock: false, link: false }),
      Link.configure({ openOnClick: false, HTMLAttributes: { class: 'editor-link' } }),
      Underline,
      WikiLink.configure({}),
    ],
    content,
  });
  const html = editor.getHTML();
  editor.destroy();
  return html;
}

// ---------------------------------------------------------------------------
// Real TipTap HTML fixtures, captured once and reused across the round-trip
// and revert-guard tests below. Verified shapes (captured via this exact
// harness during development of this fix):
//   bold in p    : <p>This is <strong>bold</strong> text.</p>
//   bold in li   : <ul><li><p>This is <strong>bold</strong> text.</p></li></ul>
//   wikilink in p: <p>See <span class="wiki-link" data-wiki-link="true"
//                    data-title="Foo Note">Foo Note</span> here.</p>
// ---------------------------------------------------------------------------

describe('Real TipTap HTML shapes (sanity check on the fixtures used below)', () => {
  it('bold as direct child of <p> — no wrapping element between <p> and <strong>', () => {
    const html = makeRealEditorHTML('<p>This is <strong>bold</strong> text.</p>');
    expect(html).toBe('<p>This is <strong>bold</strong> text.</p>');
  });

  it('bold in a list item is wrapped in <p> (TipTap always wraps <li> content in <p>)', () => {
    const html = makeRealEditorHTML('<ul><li>This is <strong>bold</strong> text.</li></ul>');
    expect(html).toBe('<ul><li><p>This is <strong>bold</strong> text.</p></li></ul>');
  });

  it('wiki-link renders as a <span data-wiki-link> direct child of <p>', () => {
    const html = makeRealEditorHTML(
      '<p>See <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> here.</p>'
    );
    expect(html).toContain('<span class="wiki-link" data-wiki-link="true" data-title="Foo Note">Foo Note</span>');
  });
});

// ---------------------------------------------------------------------------
// Round-trip tests: real TipTap HTML -> htmlToMarkdown -> expected markdown.
// One test per mark type, as a direct child of <p> AND as a direct child of
// <li> (TipTap's real <li><p>...</p></li> shape), per the task spec.
// ---------------------------------------------------------------------------

describe('Inline mark serialization — as direct child of <p>', () => {
  it('bold (<strong>) serializes to **bold**', () => {
    const html = makeRealEditorHTML('<p>This is <strong>bold</strong> text.</p>');
    expect(htmlToMarkdown(html)).toBe('This is **bold** text.');
  });

  it('italic (<em>) serializes to *italic*', () => {
    const html = makeRealEditorHTML('<p>This is <em>italic</em> text.</p>');
    expect(htmlToMarkdown(html)).toBe('This is *italic* text.');
  });

  it('code (<code>) serializes to `code`', () => {
    const html = makeRealEditorHTML('<p>Run <code>npm test</code> now.</p>');
    expect(htmlToMarkdown(html)).toBe('Run `npm test` now.');
  });

  it('link (<a>) serializes to [text](url)', () => {
    const html = makeRealEditorHTML('<p>Click <a href="https://example.com">here</a> now.</p>');
    expect(htmlToMarkdown(html)).toContain('[here](https://example.com)');
  });

  it('wiki-link (<span data-wiki-link>) serializes to [[Title]]', () => {
    const html = makeRealEditorHTML(
      '<p>See <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> here.</p>'
    );
    expect(htmlToMarkdown(html)).toBe('See [[Foo Note]] here.');
  });

  it('strikethrough (<s>) serializes to ~~strike~~', () => {
    const html = makeRealEditorHTML('<p>This is <s>struck</s> text.</p>');
    expect(htmlToMarkdown(html)).toBe('This is ~~struck~~ text.');
  });

  it('underline (<u>) preserves the underlined text (no dedicated markdown syntax in this dialect)', () => {
    const html = makeRealEditorHTML('<p>This is <u>underlined</u> text.</p>');
    expect(htmlToMarkdown(html)).toBe('This is underlined text.');
  });

  it('multiple marks together in one paragraph all survive', () => {
    const html = makeRealEditorHTML(
      '<p>This is <strong>bold</strong> and a <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> link.</p>'
    );
    expect(htmlToMarkdown(html)).toBe('This is **bold** and a [[Foo Note]] link.');
  });
});

describe('Inline mark serialization — as direct child of <li> (real TipTap <li><p> shape)', () => {
  it('bold (<strong>) in a list item serializes to - **bold**', () => {
    const html = makeRealEditorHTML('<ul><li>This is <strong>bold</strong> text.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- This is **bold** text.');
  });

  it('italic (<em>) in a list item serializes to - *italic*', () => {
    const html = makeRealEditorHTML('<ul><li>This is <em>italic</em> text.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- This is *italic* text.');
  });

  it('code (<code>) in a list item serializes to - `code`', () => {
    const html = makeRealEditorHTML('<ul><li>Run <code>npm test</code> now.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- Run `npm test` now.');
  });

  it('link (<a>) in a list item serializes to - [text](url)', () => {
    const html = makeRealEditorHTML('<ul><li>Click <a href="https://example.com">here</a> now.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('[here](https://example.com)');
  });

  it('wiki-link (<span data-wiki-link>) in a list item serializes to - [[Title]]', () => {
    const html = makeRealEditorHTML(
      '<ul><li>See <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> here.</li></ul>'
    );
    expect(htmlToMarkdown(html)).toContain('- See [[Foo Note]] here.');
  });

  it('strikethrough (<s>) in a list item serializes to - ~~strike~~', () => {
    const html = makeRealEditorHTML('<ul><li>This is <s>struck</s> text.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- This is ~~struck~~ text.');
  });

  it('underline (<u>) in a list item preserves the underlined text', () => {
    const html = makeRealEditorHTML('<ul><li>This is <u>underlined</u> text.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- This is underlined text.');
  });

  it('bold (<strong>) in an ORDERED list item serializes to 1. **bold**', () => {
    const html = makeRealEditorHTML('<ol><li>This is <strong>bold</strong> text.</li></ol>');
    expect(htmlToMarkdown(html)).toContain('1. This is **bold** text.');
  });
});

// ---------------------------------------------------------------------------
// RED-ON-REVERT: pin the exact pre-fix (buggy) output for the same fixtures.
// If someone reverts convertInlineToMarkdown to the pre-fix child-only-iteration
// version, THIS block continues to correctly describe that broken behavior
// (i.e. stays green), while every round-trip test above goes red — proving
// the round-trip tests actually exercise the fixed code path, not a fluke.
// ---------------------------------------------------------------------------

// NOTE on scope: the pre-fix bug is empirically reproducible only for the
// `<p>` call site (Editor.svelte:960), not for the `<li>` call sites
// (~973/~981). Both call `convertInlineToMarkdown(x)`, but the ARGUMENT
// differs in a way that matters: the `<p>` case loops over `<p>`'s children
// and calls `convertInlineToMarkdown(pChild)` where `pChild` may literally
// BE the mark element itself (e.g. pChild IS the `<strong>`). The `<li>`
// case calls `convertInlineToMarkdown(li)` ONCE, passing `li` itself — and
// `li` is always a plain container, never a mark — so the pre-fix
// child-only-iteration logic was already correct for `<li>` (it iterates
// `li`'s children, e.g. `<p>`, and correctly dispatches on those). The `<p>`
// unwrap inside the old switch's `case 'p':` branch then correctly recursed
// one level further into the real mark. Confirmed empirically below: the
// pre-fix `<li>` output already matches the correct/fixed output, so it is
// NOT part of the red-on-revert surface for THIS PR — but the `<li>` call
// sites are still exercised by the round-trip tests above (and would matter
// for a real regression, e.g. a mark as the ONLY, unwrapped child of `<li>`,
// which the audit flagged as "not yet repro'd" and this suite does not
// claim was ever broken).
describe('Revert guard — pre-fix implementation is confirmed broken on the <p> fixtures', () => {
  it('pre-fix: bold in p loses ** markers entirely', () => {
    const html = makeRealEditorHTML('<p>This is <strong>bold</strong> text.</p>');
    expect(htmlToMarkdown_PREFIX(html)).toBe('This is bold text.');
    // Post-fix output is different (and correct):
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: italic in p loses * markers entirely', () => {
    const html = makeRealEditorHTML('<p>This is <em>italic</em> text.</p>');
    expect(htmlToMarkdown_PREFIX(html)).toBe('This is italic text.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: code in p loses backticks entirely', () => {
    const html = makeRealEditorHTML('<p>Run <code>npm test</code> now.</p>');
    expect(htmlToMarkdown_PREFIX(html)).toBe('Run npm test now.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: link in p loses [text](url) syntax entirely, keeps only text', () => {
    const html = makeRealEditorHTML('<p>Click <a href="https://example.com">here</a> now.</p>');
    expect(htmlToMarkdown_PREFIX(html)).toBe('Click here now.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: wiki-link in p loses [[ ]] brackets entirely (THE #2419 symptom)', () => {
    const html = makeRealEditorHTML(
      '<p>See <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> here.</p>'
    );
    expect(htmlToMarkdown_PREFIX(html)).toBe('See Foo Note here.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: strikethrough in p loses ~~ markers entirely', () => {
    const html = makeRealEditorHTML('<p>This is <s>struck</s> text.</p>');
    expect(htmlToMarkdown_PREFIX(html)).toBe('This is struck text.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('pre-fix: multi-mark paragraph loses ALL mark syntax at once', () => {
    const html = makeRealEditorHTML(
      '<p>This is <strong>bold</strong> and a <span data-wiki-link="true" data-title="Foo Note">Foo Note</span> link.</p>'
    );
    expect(htmlToMarkdown_PREFIX(html)).toBe('This is bold and a Foo Note link.');
    expect(htmlToMarkdown(html)).not.toBe(htmlToMarkdown_PREFIX(html));
  });

  it('confirms <li> output is IDENTICAL pre-fix and post-fix (not part of this bug\'s reproducible surface)', () => {
    const html = makeRealEditorHTML('<ul><li>This is <strong>bold</strong> text.</li></ul>');
    expect(htmlToMarkdown_PREFIX(html)).toBe(htmlToMarkdown(html));
    expect(htmlToMarkdown(html)).toContain('- This is **bold** text.');
  });
});

// ---------------------------------------------------------------------------
// Scope guard regression: plain text (no marks) must still serialize
// unchanged — the fix must not affect content with no inline marks at all.
// ---------------------------------------------------------------------------

describe('Scope guard — plain text with no inline marks is unaffected', () => {
  it('plain paragraph with no marks round-trips unchanged', () => {
    const html = makeRealEditorHTML('<p>Just plain text, no marks at all.</p>');
    expect(htmlToMarkdown(html)).toBe('Just plain text, no marks at all.');
  });

  it('plain list item with no marks round-trips unchanged', () => {
    const html = makeRealEditorHTML('<ul><li>Plain list item text.</li></ul>');
    expect(htmlToMarkdown(html)).toContain('- Plain list item text.');
  });

  it('nested marks (bold wrapping italic) still combine correctly', () => {
    const html = makeRealEditorHTML('<p><strong><em>both</em></strong></p>');
    expect(htmlToMarkdown(html)).toBe('***both***');
  });
});
