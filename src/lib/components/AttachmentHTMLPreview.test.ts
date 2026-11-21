/**
 * AttachmentHTMLPreview.test.ts — PR6b (#401)
 *
 * Tests for the sandboxed HTML preview component.
 *
 * Tests:
 *  HTML-1  Mount → outer container (.at-html-preview) present
 *  HTML-2  <iframe> present with sandbox attribute set to empty string ('')
 *  HTML-3  <iframe> does NOT have 'allow-scripts' in sandbox value
 *  HTML-4  <iframe> does NOT have 'allow-same-origin' in sandbox value
 *  HTML-5  srcdoc attribute contains default-src 'none' (the injected CSP meta)
 *  HTML-6  srcdoc contains the mocked raw HTML body (<h1>Test</h1>), proving flow-through
 *  HTML-7  <iframe> has NO src attribute (only srcdoc) — content never as data: or asset://
 *  HTML-8  Size cap: mock returns 6 MB HTML → iframe NOT rendered, error affordance IS rendered
 *  HTML-9  Rendered .at-html-preview has NO contenteditable="true" ancestor in test render tree
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AttachmentHTMLPreview from './AttachmentHTMLPreview.svelte';

// ─── Mock @tauri-apps/api/core ────────────────────────────────────────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: any) => {
    if (cmd === 'get_attachment_data') {
      return {
        id: args.attachmentId,
        mime_type: 'text/html',
        original_filename: 'test.html',
        data: btoa('<html><body><h1>Test</h1></body></html>'),
      };
    }
  }),
}));

import { invoke } from '@tauri-apps/api/core';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function renderPreview(overrides: Record<string, any> = {}) {
  return render(AttachmentHTMLPreview, {
    props: {
      attachmentId: 'test-html-001',
      filename: 'test.html',
      ...overrides,
    },
  });
}

// Flush async (onMount invoke) + Svelte 5 state update.
async function flushAsync() {
  await Promise.resolve(); // drain microtask queue
  await tick();            // flush Svelte reactive state
}

// ─── HTML-1: Outer container present ─────────────────────────────────────────

describe('HTML-1 — outer container (.at-html-preview) present', () => {
  it('renders the .at-html-preview outer wrapper', () => {
    const { container } = renderPreview();
    const wrapper = container.querySelector('.at-html-preview');
    expect(wrapper).not.toBeNull();
  });
});

// ─── HTML-2: <iframe> with sandbox="" ────────────────────────────────────────

describe('HTML-2 — <iframe> present with sandbox attribute set to empty string', () => {
  it('renders an <iframe> element after onMount resolves', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    expect(iframe).not.toBeNull();
  });

  it('<iframe> has sandbox attribute set to empty string', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    expect(iframe?.getAttribute('sandbox')).toBe('');
  });
});

// ─── HTML-3: No allow-scripts in sandbox ─────────────────────────────────────

describe('HTML-3 — <iframe> does NOT have allow-scripts in sandbox value', () => {
  it('sandbox attribute does not contain allow-scripts', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const sandbox = iframe?.getAttribute('sandbox') ?? '';
    expect(sandbox).not.toContain('allow-scripts');
  });
});

// ─── HTML-4: No allow-same-origin in sandbox ─────────────────────────────────

describe('HTML-4 — <iframe> does NOT have allow-same-origin in sandbox value', () => {
  it('sandbox attribute does not contain allow-same-origin', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const sandbox = iframe?.getAttribute('sandbox') ?? '';
    expect(sandbox).not.toContain('allow-same-origin');
  });
});

// ─── HTML-5: srcdoc contains CSP meta ────────────────────────────────────────

describe("HTML-5 — srcdoc attribute contains default-src 'none'", () => {
  it("srcdoc contains the injected CSP meta with default-src 'none'", async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const srcdoc = iframe?.getAttribute('srcdoc') ?? '';
    expect(srcdoc).toContain("default-src 'none'");
  });

  it('srcdoc contains the full Content-Security-Policy meta tag', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const srcdoc = iframe?.getAttribute('srcdoc') ?? '';
    expect(srcdoc).toContain('Content-Security-Policy');
  });
});

// ─── HTML-6: srcdoc contains mocked raw HTML body ────────────────────────────

describe('HTML-6 — srcdoc contains the mocked raw HTML body (flow-through from get_attachment_data)', () => {
  it('srcdoc contains the mocked <h1>Test</h1> content', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const srcdoc = iframe?.getAttribute('srcdoc') ?? '';
    expect(srcdoc).toContain('<h1>Test</h1>');
  });

  it('srcdoc contains the full mocked HTML body', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const srcdoc = iframe?.getAttribute('srcdoc') ?? '';
    expect(srcdoc).toContain('<html><body><h1>Test</h1></body></html>');
  });
});

// ─── HTML-7: <iframe> has NO src attribute ────────────────────────────────────

describe('HTML-7 — <iframe> has NO src attribute (only srcdoc)', () => {
  it('<iframe> does not have a src attribute', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    expect(iframe?.hasAttribute('src')).toBe(false);
  });

  it('<iframe> src attribute is not set to data: or asset://', async () => {
    const { container } = renderPreview();
    await flushAsync();
    const iframe = container.querySelector('iframe');
    const src = iframe?.getAttribute('src') ?? '';
    expect(src).not.toMatch(/^data:/);
    expect(src).not.toMatch(/^asset:\/\//);
  });
});

// ─── HTML-8: Size cap enforcement ────────────────────────────────────────────

describe('HTML-8 — size cap: 6 MB HTML → iframe NOT rendered, error affordance IS rendered', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockClear();
  });

  it('does not render <iframe> when file exceeds 5 MB hard cap', async () => {
    // Generate a 6 MB base64-encoded string
    // 6 MB of raw bytes = ~8 MB base64; we need the decoded size to be ~6.3 MB
    // Use a string that when base64-decoded produces > 5 MB
    const largeBuf = new Uint8Array(6 * 1024 * 1024 + 100);
    largeBuf.fill(65); // fill with 'A'
    const chunks: string[] = [];
    for (let i = 0; i < largeBuf.length; i += 8192) {
      chunks.push(String.fromCharCode(...largeBuf.subarray(i, i + 8192)));
    }
    const largeBase64 = btoa(chunks.join(''));

    vi.mocked(invoke).mockResolvedValueOnce({
      id: 'large-html-001',
      mime_type: 'text/html',
      original_filename: 'large.html',
      data: largeBase64,
    });

    const { container } = renderPreview({ attachmentId: 'large-html-001', filename: 'large.html' });
    await flushAsync();

    const iframe = container.querySelector('iframe');
    expect(iframe).toBeNull();
  });

  it('renders the too-large error affordance with "too large" text when file exceeds 5 MB hard cap', async () => {
    const largeBuf = new Uint8Array(6 * 1024 * 1024 + 100);
    largeBuf.fill(65);
    const chunks: string[] = [];
    for (let i = 0; i < largeBuf.length; i += 8192) {
      chunks.push(String.fromCharCode(...largeBuf.subarray(i, i + 8192)));
    }
    const largeBase64 = btoa(chunks.join(''));

    vi.mocked(invoke).mockResolvedValueOnce({
      id: 'large-html-002',
      mime_type: 'text/html',
      original_filename: 'large.html',
      data: largeBase64,
    });

    const { container } = renderPreview({ attachmentId: 'large-html-002', filename: 'large.html' });
    await flushAsync();

    const errorEl = container.querySelector('.at-html-error');
    expect(errorEl).not.toBeNull();
    expect(errorEl?.textContent).toMatch(/too large/i);
  });
});

// ─── HTML-9: No contenteditable="true" ancestor ───────────────────────────────

describe('HTML-9 — .at-html-preview has no contenteditable="true" ancestor in the test render tree', () => {
  it('does not have contenteditable="true" on the preview container or its ancestors', async () => {
    const { container } = renderPreview();
    await flushAsync();

    const preview = container.querySelector('.at-html-preview');
    expect(preview).not.toBeNull();

    // Walk up from the preview element to document root — no contenteditable="true" should appear
    let el: Element | null = preview;
    while (el) {
      expect(el.getAttribute('contenteditable')).not.toBe('true');
      el = el.parentElement;
    }
  });

  it('the raw HTML string does not appear as text content in the parent container', async () => {
    const { container } = renderPreview();
    await flushAsync();

    // The raw HTML content must be inside the iframe's srcdoc, not injected as text into the DOM
    const textContent = container.textContent ?? '';
    // The mocked raw HTML body tags should not appear as rendered text nodes in the host DOM
    expect(textContent).not.toContain('<h1>Test</h1>');
  });
});
