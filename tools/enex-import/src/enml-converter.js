/**
 * ENML → Markdown converter.
 * 
 * Converts Evernote's ENML (a subset of XHTML) to clean Markdown using Turndown,
 * with special handling for Evernote-specific elements like:
 *   - <en-note> (document root)
 *   - <en-media> (embedded attachments)
 *   - <en-todo> (checklists)
 *   - <en-crypt> (encrypted content)
 */

import TurndownService from 'turndown';
import { gfm } from 'turndown-plugin-gfm';

/**
 * Create a configured Turndown instance with Evernote-specific rules.
 * 
 * @param {Map<string, object>} resourceMap - Map of MD5 hash → resource info
 * @param {string} attachmentDir - Relative path to attachments directory
 * @returns {TurndownService}
 */
export function createConverter(resourceMap, attachmentDir) {
  const turndown = new TurndownService({
    headingStyle: 'atx',
    hr: '---',
    bulletListMarker: '-',
    codeBlockStyle: 'fenced',
    fence: '```',
    emDelimiter: '*',
    strongDelimiter: '**',
    linkStyle: 'inlined',
    linkReferenceStyle: 'full',
    blankReplacement: (content, node) => {
      // Preserve line breaks for block elements
      if (node.nodeName === 'DIV' || node.nodeName === 'P' || node.nodeName === 'BR') {
        return '\n';
      }
      return '';
    },
  });

  // Enable GFM (tables, strikethrough, task lists)
  turndown.use(gfm);

  // --- Evernote-specific rules ---

  // <en-note> — just unwrap it
  turndown.addRule('en-note', {
    filter: (node) => node.nodeName === 'EN-NOTE',
    replacement: (content) => content,
  });

  // <en-todo> — convert to Markdown checkboxes
  turndown.addRule('en-todo', {
    filter: (node) => node.nodeName === 'EN-TODO',
    replacement: (content, node) => {
      const checked = node.getAttribute('checked') === 'true';
      return checked ? '[x] ' : '[ ] ';
    },
  });

  // <en-media> — convert to Markdown image/link references
  turndown.addRule('en-media', {
    filter: (node) => node.nodeName === 'EN-MEDIA',
    replacement: (content, node) => {
      const hash = node.getAttribute('hash');
      const type = node.getAttribute('type') || '';

      if (!hash) return '';

      const resource = resourceMap.get(hash);
      if (!resource) {
        return `<!-- missing attachment: ${hash} -->`;
      }

      const filename = resource.savedFilename || resource.filename || `${hash}`;
      const path = `${attachmentDir}/${filename}`;

      if (type.startsWith('image/')) {
        const alt = resource.filename || 'image';
        return `![${alt}](${path})`;
      } else {
        // Non-image attachment: link to it
        const label = resource.filename || filename;
        return `[${label}](${path})`;
      }
    },
  });

  // <en-crypt> — mark as encrypted (can't decrypt without user's key)
  turndown.addRule('en-crypt', {
    filter: (node) => node.nodeName === 'EN-CRYPT',
    replacement: () => {
      return '> ⚠️ *Encrypted content — could not be imported*\n';
    },
  });

  // Evernote uses <div> as line separators heavily
  turndown.addRule('evernote-div', {
    filter: (node) => {
      return node.nodeName === 'DIV' && !node.getAttribute('class');
    },
    replacement: (content) => {
      return '\n' + content.trim() + '\n';
    },
  });

  // Clean up font/span styling (Evernote adds lots of inline styles)
  turndown.addRule('strip-styling-spans', {
    filter: (node) => {
      return (node.nodeName === 'SPAN' || node.nodeName === 'FONT') &&
        !node.getAttribute('class');
    },
    replacement: (content) => content,
  });

  return turndown;
}

/**
 * Pre-process ENML to convert Evernote custom elements to standard HTML
 * that Turndown can handle. Turndown's DOM parser ignores unknown void elements.
 * 
 * @param {string} enml - Raw ENML
 * @param {Map<string, object>} resourceMap - Resource map
 * @param {string} attachmentDir - Attachment directory path
 * @returns {string} HTML with custom elements replaced
 */
function preprocessEnml(enml, resourceMap, attachmentDir) {
  let html = enml;

  // Strip XML declaration and DOCTYPE
  html = html
    .replace(/<\?xml[^>]*\?>/gi, '')
    .replace(/<!DOCTYPE[^>]*>/gi, '');

  // Replace <en-note> with <div>
  html = html.replace(/<en-note[^>]*>/gi, '<div>');
  html = html.replace(/<\/en-note>/gi, '</div>');

  // Replace <en-todo> with placeholder strings that postProcess will convert
  // Using Unicode markers that Turndown won't escape
  html = html.replace(/<en-todo\s+checked="true"\s*\/?>/gi, '☑TODOCHECKED☑ ');
  html = html.replace(/<en-todo\s+checked="false"\s*\/?>/gi, '☐TODOUNCHECKED☐ ');
  html = html.replace(/<en-todo\s*\/?>/gi, '☐TODOUNCHECKED☐ ');

  // Replace <en-media> with appropriate HTML
  html = html.replace(/<en-media([^>]*?)\/?\s*>/gi, (match, attrs) => {
    const hashMatch = attrs.match(/hash="([^"]+)"/);
    const typeMatch = attrs.match(/type="([^"]+)"/);
    const hash = hashMatch ? hashMatch[1] : '';
    const type = typeMatch ? typeMatch[1] : '';

    if (!hash) return '';

    const resource = resourceMap.get(hash);
    if (!resource) {
      return `<em>⚠️ missing attachment: ${hash}</em>`;
    }

    const filename = resource.savedFilename || resource.filename || hash;
    const path = `${attachmentDir}/${filename}`;

    if (type.startsWith('image/')) {
      const alt = resource.filename || 'image';
      return `<img src="${path}" alt="${alt}">`;
    } else {
      const label = resource.filename || filename;
      return `<a href="${path}">${label}</a>`;
    }
  });
  // Handle closing en-media tags if any
  html = html.replace(/<\/en-media>/gi, '');

  // Replace <en-crypt> with warning
  html = html.replace(/<en-crypt[^>]*>[\s\S]*?<\/en-crypt>/gi,
    '<blockquote>⚠️ <em>Encrypted content — could not be imported</em></blockquote>');

  return html;
}

/**
 * Convert ENML content to Markdown.
 * 
 * @param {string} enml - Raw ENML string (the content of <content> in ENEX)
 * @param {Map<string, object>} resourceMap - Map of MD5 hash → resource info
 * @param {string} attachmentDir - Relative path to attachments directory
 * @returns {string} Markdown content
 */
export function convertEnmlToMarkdown(enml, resourceMap, attachmentDir) {
  if (!enml || !enml.trim()) return '';

  const converter = createConverter(resourceMap, attachmentDir);

  // Pre-process ENML custom elements into standard HTML
  const html = preprocessEnml(enml, resourceMap, attachmentDir);

  // Convert to Markdown
  let markdown = converter.turndown(html);

  // Post-processing cleanup
  markdown = postProcess(markdown);

  return markdown;
}

/**
 * Post-process Markdown to clean up conversion artifacts.
 */
function postProcess(md) {
  return md
    // Convert checkbox placeholders to proper Markdown
    .replace(/☑TODOCHECKED☑\s*/g, '- [x] ')
    .replace(/☐TODOUNCHECKED☐\s*/g, '- [ ] ')
    // Remove excessive blank lines (3+ → 2)
    .replace(/\n{3,}/g, '\n\n')
    // Remove trailing whitespace on lines
    .replace(/[ \t]+$/gm, '')
    // Ensure file ends with single newline
    .trim() + '\n';
}

export default { convertEnmlToMarkdown, createConverter };
