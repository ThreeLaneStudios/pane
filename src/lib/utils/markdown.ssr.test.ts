// @vitest-environment node

import { describe, expect, it } from 'vitest';

describe('renderMarkdown without a browser DOM', () => {
  it('imports and renders safe Markdown without document or window globals', async () => {
    expect(typeof document).toBe('undefined');
    expect(typeof window === 'undefined' || window.document === undefined).toBe(true);


    const { renderMarkdown } = await import('./markdown');
    const html = renderMarkdown('**safe** <script>alert(1)</script> [bad](javascript:alert(2))');

    expect(html).toContain('<strong>safe</strong>');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('href=');
  });
});
