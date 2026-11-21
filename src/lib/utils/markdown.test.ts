import { describe, expect, it } from 'vitest';
import { highlightText, renderMarkdown } from './markdown';

function render(markdown: string): HTMLElement {
  const container = document.createElement('div');
  container.innerHTML = renderMarkdown(markdown);
  return container;
}

describe('renderMarkdown', () => {
  it('removes event attributes and inline CSS', () => {
    const container = render('<span onclick="alert(1)" onmouseover="alert(2)" style="background:url(javascript:alert(3))">Click</span>');

    expect(container.querySelector('[onclick], [onmouseover], [style]')).toBeNull();
    expect(container.textContent).toContain('Click');
  });

  it('removes script, SVG, and MathML elements', () => {
    const container = render([
      '<script>alert(1)</script>',
      '<svg><a xlink:href="javascript:alert(2)">svg</a></svg>',
      '<math><mtext><img src=x onerror=alert(3)></mtext></math>',
    ].join(''));

    expect(container.querySelector('script, svg, math, mtext')).toBeNull();
  });

  it('handles malformed and nested malicious tags as inert text', () => {
    const container = render('<scr<script>ipt>alert(1)</scr</script>ipt><a href="jav&#x61;script:alert(2)">bad');

    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('[onclick], [onerror]')).toBeNull();
    expect(container.querySelector('a[href]')).toBeNull();
  });

  it('rejects javascript and data URLs, including obfuscated schemes', () => {
    const container = render([
      '[javascript](javascript:alert(1))',
      '[data](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)',
      '[obfuscated](java%0ascript:alert(2))',
    ].join('\n\n'));

    expect(container.querySelectorAll('a[href]')).toHaveLength(0);
    expect(container.textContent).toContain('javascript');
    expect(container.textContent).toContain('data');
  });

  it('removes forms, embeds, frames, objects, and passive image content', () => {
    const container = render([
      '<form action="https://example.com"><input name="secret"><button>Send</button></form>',
      '<iframe src="https://example.com"></iframe>',
      '<object data="https://example.com/file"></object>',
      '<embed src="https://example.com/file">',
      '![remote image](https://example.com/tracker.png)',
      '<img src="https://example.com/tracker.png" alt="tracker">',
    ].join('\n'));

    expect(container.querySelector('form, input, button, iframe, object, embed, img')).toBeNull();
    expect(container.textContent).toContain('remote image');
  });

  it('preserves normal Markdown formatting, tables, code, and safe links', () => {
    const container = render([
      '## Heading',
      '',
      '**bold** and *italic* and ~~deleted~~ and `code`',
      '',
      '- one',
      '- two',
      '- [x] complete',
      '',
      '| Name | Value |',
      '| --- | --- |',
      '| Pane | Safe |',
      '',
      '[Pane](https://pane.so "Pane site")',
    ].join('\n'));

    expect(container.querySelector('h2')?.textContent).toBe('Heading');
    expect(container.querySelector('strong')?.textContent).toBe('bold');
    expect(container.querySelector('em')?.textContent).toBe('italic');
    expect(container.querySelector('del')?.textContent).toBe('deleted');
    expect(container.querySelector('code')?.textContent).toBe('code');
    expect([...container.querySelectorAll('li')].map((item) => item.textContent?.trim())).toEqual(['one', 'two', 'complete']);
    expect(container.querySelector('input[type="checkbox"]')?.hasAttribute('disabled')).toBe(true);
    expect(container.querySelector('input[type="checkbox"]')?.hasAttribute('checked')).toBe(true);
    expect(container.querySelector('table td')?.textContent).toBe('Pane');
    expect(container.querySelector('a')?.getAttribute('href')).toBe('https://pane.so');
    expect(container.querySelector('a')?.getAttribute('title')).toBe('Pane site');
  });

  // #3151 PR1 Commit D: the job-output link mechanism. A job turn's
  // `create_note`/`create_sheet` tool call appends `[title](pane://note/
  // {id})` (`output_link_token`, `ai/tools.rs`) to the job session's own
  // reply and the origin receipt — this is the renderer half: the link
  // must survive sanitization as a real `<a>`, not be stripped (the
  // "visible and wrong" / "silently gone" failure mode this sprint's
  // task warns about is a job output the user literally cannot click).
  it('renders a pane://note/ job-output link as a real, clickable anchor', () => {
    const container = render('[Research Note](pane://note/abc-123)');

    const anchor = container.querySelector('a[href]');
    expect(anchor).not.toBeNull();
    expect(anchor?.getAttribute('href')).toBe('pane://note/abc-123');
    expect(anchor?.textContent).toBe('Research Note');
    // Mutation check (inverse of the javascript/data-URL rejection test
    // above): if `pane:` were NOT added to `SAFE_URL`, this would reproduce
    // that test's own "zero real anchors, raw text survives" shape instead.
    expect(container.textContent).not.toContain('[Research Note]');
  });

  // #3151 PR3 (Reed's PR1 note): the allowlist is the exact `pane://note/`
  // shape `output_link_token` generates, not a bare `pane:` scheme —
  // anything else under `pane:` must be stripped the same way an
  // unlisted scheme always is. Cosmetic in effect today (the click
  // handler's own exact-prefix check already no-ops any other `pane://`
  // link), but the allowlist itself must not be wider than the one shape
  // it exists for.
  it('does NOT allowlist a pane: URL outside the pane://note/ shape', () => {
    const container = render('[Other](pane://other/abc-123)');
    const anchor = container.querySelector('a[href]');
    expect(anchor).toBeNull();
  });

  it('does NOT allowlist a bare pane: scheme with no pane://note/ path', () => {
    const container = render('[Bare](pane:evil)');
    const anchor = container.querySelector('a[href]');
    expect(anchor).toBeNull();
  });
});

describe('highlightText', () => {
  it('highlights matches while escaping HTML in external text', () => {
    const html = highlightText('<img src=x onerror=alert(1)> Pane <script>alert(2)</script>', 'Pane');
    const container = document.createElement('div');
    container.innerHTML = html;

    expect(container.querySelector('img, script')).toBeNull();
    expect(container.querySelector('mark')?.textContent).toBe('Pane');
    expect(container.textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('escapes text even when there is no query', () => {
    expect(highlightText('<svg onload=alert(1)>', '')).toBe('&lt;svg onload=alert(1)&gt;');
  });

  it('treats regular-expression characters in the query literally', () => {
    expect(highlightText('model [pro] model', '[pro]')).toBe('model <mark>[pro]</mark> model');
  });
});
