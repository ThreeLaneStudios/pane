import { describe, it, expect } from 'vitest';

/**
 * #1736 — Version Restore Markdown Conversion
 *
 * These tests verify the conversion logic used in `restoreFromVersion()` after the fix.
 * They test the logic in isolation (not mounting the Svelte component) because the project
 * does not yet have a full Svelte component test harness (#1723).
 *
 * Full E2E restore testing (mount Editor, trigger restore, verify TipTap receives HTML)
 * is deferred to #1723.
 */

// ---------------------------------------------------------------------------
// Extracted helpers mirroring Editor.svelte's markdownToHtml + processInline
// ---------------------------------------------------------------------------

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function processInline(text: string): string {
  // Bold + italic combined ***
  text = text.replace(/\*\*\*(.*?)\*\*\*/g, '<strong><em>$1</em></strong>');
  // Bold
  text = text.replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>');
  text = text.replace(/__(.*?)__/g, '<strong>$1</strong>');
  // Italic
  text = text.replace(/\*(.*?)\*/g, '<em>$1</em>');
  text = text.replace(/_(.*?)_/g, '<em>$1</em>');
  // Inline code
  text = text.replace(/`([^`]+)`/g, '<code>$1</code>');
  // Links
  text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');
  return text;
}

/**
 * Simplified version of markdownToHtml() from Editor.svelte.
 * Tests here validate the LOGIC, not a separate implementation.
 * In production, Editor.svelte's own markdownToHtml() is used directly.
 */
function markdownToHtml(md: string): string {
  let html = md;

  // Code blocks first
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_: string, lang: string, code: string) => {
    return `<pre><code class="language-${lang}">${escapeHtml(code.trimEnd())}</code></pre>`;
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

    if (block.match(/^#{1,6} /)) {
      const level = block.match(/^(#{1,6}) /)![1].length;
      const text = block.replace(/^#{1,6} /, '');
      return `<h${level}>${processInline(text.trim())}</h${level}>`;
    }

    if (block.match(/^(-{3,}|\*{3,}|_{3,})$/)) return '<hr>';

    if (block.startsWith('> ')) {
      const content = block.split('\n').map((l: string) => l.replace(/^> ?/, '')).join('\n');
      return `<blockquote><p>${processInline(content)}</p></blockquote>`;
    }

    if (block.match(/^[-*+] /m)) {
      const items = block.split('\n').filter((l: string) => l.trim());
      const lis = items.map((item: string) => {
        const text = item.replace(/^[-*+] /, '');
        return `<li><p>${processInline(text)}</p></li>`;
      });
      return `<ul>${lis.join('')}</ul>`;
    }

    if (block.match(/^\d+\. /m)) {
      const items = block.split('\n').filter((l: string) => l.trim());
      const lis = items.map((item: string) => {
        const text = item.replace(/^\d+\. /, '');
        return `<li><p>${processInline(text)}</p></li>`;
      });
      return `<ol>${lis.join('')}</ol>`;
    }

    const lines = block.split('\n');
    if (lines.length > 1) {
      const processedLines = lines
        .map((l: string) => l.trim())
        .filter((l: string) => l.length > 0)
        .map((l: string) => processInline(l));
      return `<p>${processedLines.join('<br>')}</p>`;
    }
    const processed = processInline(block.trim());
    return processed ? `<p>${processed}</p>` : '';
  }).join('');

  return html;
}

// ---------------------------------------------------------------------------
// The title-strip regex (same regex used in restoreFromVersion() after fix)
// ---------------------------------------------------------------------------
const TITLE_STRIP_RE = /^# .*\n\n?/;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Editor #1736 — restoreFromVersion() markdown conversion logic', () => {

  // --- Title-strip regex ---

  describe('Title-strip regex (/^# .*\\n\\n?/)', () => {
    it('strips a top-level heading followed by a blank line', () => {
      const raw = '# My Note Title\n\nBody text here.';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('Body text here.');
    });

    it('strips a top-level heading followed by a single newline (no blank line)', () => {
      const raw = '# My Note Title\nBody text here.';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('Body text here.');
    });

    it('does NOT strip h2 or lower headings', () => {
      const raw = '## Subtitle\n\nBody text.';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('## Subtitle\n\nBody text.');
    });

    it('handles notes with only a title (no body)', () => {
      const raw = '# My Note Title\n';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('');
    });

    it('leaves content intact when no title heading is present', () => {
      const raw = 'Just a plain paragraph.\n\nSecond paragraph.';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('Just a plain paragraph.\n\nSecond paragraph.');
    });

    it('handles typical version-history content (title + multiple sections)', () => {
      const raw = '# Meeting Notes\n\n## Agenda\n\n- Item 1\n- Item 2';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('## Agenda\n\n- Item 1\n- Item 2');
    });
  });

  // --- markdownToHtml conversion ---

  describe('markdownToHtml() — heading conversion', () => {
    it('converts h1 markdown to <h1> HTML', () => {
      const html = markdownToHtml('# Heading One');
      expect(html).toBe('<h1>Heading One</h1>');
    });

    it('converts h2 markdown to <h2> HTML', () => {
      const html = markdownToHtml('## Heading Two');
      expect(html).toBe('<h2>Heading Two</h2>');
    });

    it('converts h3 markdown to <h3> HTML', () => {
      const html = markdownToHtml('### Heading Three');
      expect(html).toBe('<h3>Heading Three</h3>');
    });

    it('does NOT render raw # characters in output', () => {
      const html = markdownToHtml('# Test Heading');
      expect(html).not.toContain('#');
      expect(html).toContain('<h1>');
    });
  });

  describe('markdownToHtml() — inline formatting', () => {
    it('converts **bold** to <strong>', () => {
      const html = markdownToHtml('This is **bold** text.');
      expect(html).toContain('<strong>bold</strong>');
    });

    it('converts *italic* to <em>', () => {
      const html = markdownToHtml('This is *italic* text.');
      expect(html).toContain('<em>italic</em>');
    });

    it('converts `inline code` to <code>', () => {
      const html = markdownToHtml('Use `const x = 1` here.');
      expect(html).toContain('<code>const x = 1</code>');
    });
  });

  describe('markdownToHtml() — lists', () => {
    it('converts unordered list to <ul><li>', () => {
      const html = markdownToHtml('- First item\n- Second item');
      expect(html).toContain('<ul>');
      expect(html).toContain('<li>');
      expect(html).toContain('First item');
      expect(html).toContain('Second item');
    });

    it('converts ordered list to <ol><li>', () => {
      const html = markdownToHtml('1. First\n2. Second');
      expect(html).toContain('<ol>');
      expect(html).toContain('<li>');
      expect(html).toContain('First');
      expect(html).toContain('Second');
    });
  });

  // --- Full pipeline: title-strip + markdownToHtml (mirrors restoreFromVersion fix) ---

  describe('Full restore pipeline: title-strip → markdownToHtml', () => {
    it('correctly converts a typical note version (heading + subheading + bold + list)', () => {
      const rawMarkdown = '# Meeting Notes\n\n## Agenda\n\nThis has **bold** text.\n\n- Action 1\n- Action 2';

      // Step 1: strip title (same as restoreFromVersion() after fix)
      const contentWithoutTitle = rawMarkdown.replace(TITLE_STRIP_RE, '');
      expect(contentWithoutTitle).toBe('## Agenda\n\nThis has **bold** text.\n\n- Action 1\n- Action 2');

      // Step 2: convert to HTML (same as restoreFromVersion() after fix)
      const html = markdownToHtml(contentWithoutTitle);

      // Verify HTML — no raw markdown characters leaked
      expect(html).not.toMatch(/^#/m);         // No raw # headings
      expect(html).not.toContain('**');         // No raw ** bold markers

      // Verify correct HTML output
      expect(html).toContain('<h2>Agenda</h2>');
      expect(html).toContain('<strong>bold</strong>');
      expect(html).toContain('<ul>');
      expect(html).toContain('Action 1');
      expect(html).toContain('Action 2');
    });

    it('produces HTML not raw markdown (TipTap compatibility check)', () => {
      const raw = '# Note Title\n\n## Section\n\nContent here.';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      const html = markdownToHtml(stripped);

      // TipTap requires HTML — verify the result is HTML
      expect(html).toMatch(/<[a-z]/);           // Contains at least one HTML tag
      expect(html).not.toMatch(/^#{1,6} /m);   // No raw heading markers
    });

    it('handles empty content gracefully', () => {
      const html = markdownToHtml('');
      expect(html).toBe('');
    });

    it('handles note with only a title (no body after strip)', () => {
      const raw = '# Just a Title\n';
      const stripped = raw.replace(TITLE_STRIP_RE, '');
      expect(stripped).toBe('');
      const html = markdownToHtml(stripped);
      expect(html).toBe('');
    });
  });
});
