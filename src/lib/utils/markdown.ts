import createDOMPurify from 'dompurify';
import { marked, Renderer } from 'marked';

marked.setOptions({
  breaks: true,
  gfm: true,
});

const ALLOWED_TAGS = [
  'a',
  'blockquote',
  'br',
  'code',
  'del',
  'em',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'hr',
  'input',
  'li',
  'ol',
  'p',
  'pre',
  'span',
  'strong',
  'table',
  'tbody',
  'td',
  'th',
  'thead',
  'tr',
  'ul',
] as const;

const ALLOWED_CLASSES = new Set([
  'priority-pill',
  'priority--critical',
  'priority--high',
  'priority--low',
  'priority--medium',
  'status-dot',
  'status-indicator',
  'status--blocked',
  'status--done',
  'status--due-today',
  'status--on-track',
  'status--overdue',
]);

// #3151 PR1 Commit D: `pane:` added so a job-output link rendered as
// `[title](pane://note/{id})` (`output_link_token`, `ai/tools.rs`) survives
// both this module's own `renderer.link` override AND DOMPurify's
// `uponSanitizeAttribute` hook below (both gate on this SAME regex via
// `safeUrl`) — without it, DOMPurify strips the `href` entirely (`ALLOW_
// UNKNOWN_PROTOCOLS: false` has no bearing here; `safeUrl`'s own check
// runs first and is what actually decides). `ChatPanel.svelte`'s
// `handleLinkClick` gets the one new branch that intercepts this scheme
// instead of falling through to "open externally" — see that function's
// own comment.
//
// #3151 PR3 (Reed's PR1 note): narrowed from a bare `pane:` prefix (which
// would have allowlisted ANY `pane:...` scheme, not just the one shape
// `output_link_token` ever generates) to the exact `pane://note/` shape
// plus a non-empty id — matching `PANE_NOTE_LINK_PREFIX`
// (`ChatPanel.svelte`) byte-for-byte. Cosmetic today (that click
// handler's own exact-prefix check already makes any OTHER `pane://`
// link a no-op — nothing renders differently, nothing becomes newly
// clickable), but a wider-than-needed scheme in a DOMPurify allowlist
// shouldn't stand regardless of whether anything currently exploits the
// slack.
const SAFE_URL = /^(?:(?:https?|mailto):|pane:\/\/note\/[^\s]+|[#/?]|\.\.?\/)/i;
const renderer = new Renderer();
let purifier: ReturnType<typeof createDOMPurify> | undefined;

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function safeUrl(url: string): string | null {
  // Removing ASCII whitespace/control characters prevents scheme obfuscation such as
  // "java\nscript:" while preserving whitespace in ordinary relative URLs as encoded text.
  const normalizedScheme = url.trim().replace(/[\u0000-\u0020\u007f]+/g, '');
  return SAFE_URL.test(normalizedScheme) ? url : null;
}

// Raw HTML and images are not needed to represent normal Markdown text and can load
// remote content. Escaping them before sanitization also makes the SSR path safe by
// construction when no browser DOM is available.
renderer.html = ({ text }) => escapeHtml(text);
renderer.image = ({ text }) => escapeHtml(text);
renderer.link = ({ href, title, tokens }) => {
  const text = renderer.parser.parseInline(tokens);
  const sanitizedHref = safeUrl(href);
  if (!sanitizedHref) return text;

  const titleAttribute = title ? ` title="${escapeHtml(title)}"` : '';
  return `<a href="${escapeHtml(sanitizedHref)}"${titleAttribute}>${text}</a>`;
};

function getPurifier(): ReturnType<typeof createDOMPurify> | undefined {
  if (purifier) return purifier;
  if (typeof window === 'undefined' || !window.document) return undefined;

  purifier = createDOMPurify(window);
  purifier.addHook('uponSanitizeAttribute', (_node, data) => {
    if (data.attrName === 'class') {
      const safeClasses = data.attrValue
        .split(/\s+/)
        .filter((className) => ALLOWED_CLASSES.has(className) || /^language-[\w-]+$/.test(className));

      if (safeClasses.length === 0) {
        data.keepAttr = false;
      } else {
        data.attrValue = safeClasses.join(' ');
      }
    }

    if (data.attrName === 'href' && !safeUrl(data.attrValue)) {
      data.keepAttr = false;
    }
  });

  return purifier;
}

/** Convert untrusted Markdown to allowlisted HTML that is safe to inject into the DOM. */
export function renderMarkdown(markdown: string): string {
  const html = marked.parse(markdown, { async: false, renderer });
  const domPurify = getPurifier();

  if (!domPurify) return html;

  return domPurify.sanitize(html, {
    ALLOWED_TAGS: [...ALLOWED_TAGS],
    ALLOWED_ATTR: ['checked', 'class', 'disabled', 'href', 'start', 'title', 'type'],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    ALLOW_UNKNOWN_PROTOCOLS: false,
  });
}

/** Escape untrusted text and wrap matching portions in a single safe `<mark>` tag. */
export function highlightText(text: string, query: string): string {
  const trimmedQuery = query.trim();
  if (!trimmedQuery) return escapeHtml(text);

  const pattern = new RegExp(trimmedQuery.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
  let result = '';
  let previousIndex = 0;

  for (const match of text.matchAll(pattern)) {
    const index = match.index ?? 0;
    result += escapeHtml(text.slice(previousIndex, index));
    result += `<mark>${escapeHtml(match[0])}</mark>`;
    previousIndex = index + match[0].length;
  }

  return result + escapeHtml(text.slice(previousIndex));
}
