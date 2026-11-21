/**
 * AttachmentPdfReader.test.ts — PR6a (#401)
 *
 * Tests for the inline PDF reader component.
 *
 * Tests:
 *  PDF-1  Mount → outer container (.at-pdf-reader) present
 *  PDF-2  <object type="application/pdf"> present after src resolves
 *  PDF-3  Src resolution: resolveAttachmentSrc called with passed attachmentId
 *  PDF-4  <object data> receives the resolved asset:// URL from the mock
 *  PDF-5  No base64: <object data> starts with asset://, never data:
 *  PDF-6  Empty resolve (mock returns '') → no <object> rendered (placeholder shown)
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AttachmentPdfReader from './AttachmentPdfReader.svelte';

// ─── Mock resolveAttachmentSrc ────────────────────────────────────────────────
vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn(async (id: string) => `asset://localhost/attachments/${id}.pdf`),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
}));

import { resolveAttachmentSrc } from '$lib/api';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function renderReader(overrides: Record<string, any> = {}) {
  return render(AttachmentPdfReader, {
    props: {
      attachmentId: 'test-pdf-001',
      filename: 'document.pdf',
      ...overrides,
    },
  });
}

// Flush async (onMount resolveAttachmentSrc) + Svelte 5 state update.
async function flushAsync() {
  await Promise.resolve(); // drain microtask queue
  await tick();            // flush Svelte reactive state
}

// ─── PDF-1: Outer container present ──────────────────────────────────────────

describe('PDF-1 — outer container (.at-pdf-reader) present', () => {
  it('renders the .at-pdf-reader outer wrapper', () => {
    const { container } = renderReader();
    const wrapper = container.querySelector('.at-pdf-reader');
    expect(wrapper).not.toBeNull();
  });

  it('renders the .at-pdf-ratio aspect-ratio container', () => {
    const { container } = renderReader();
    expect(container.querySelector('.at-pdf-ratio')).not.toBeNull();
  });
});

// ─── PDF-2: <object type="application/pdf"> present ──────────────────────────

describe('PDF-2 — <object type="application/pdf"> present after src resolves', () => {
  it('renders an <object> element after onMount resolves', async () => {
    const { container } = renderReader();
    await flushAsync();
    const obj = container.querySelector('object');
    expect(obj).not.toBeNull();
  });

  it('<object> has type="application/pdf"', async () => {
    const { container } = renderReader();
    await flushAsync();
    const obj = container.querySelector('object');
    expect(obj?.getAttribute('type')).toBe('application/pdf');
  });
});

// ─── PDF-3: Src resolution ────────────────────────────────────────────────────

describe('PDF-3 — src resolution: resolveAttachmentSrc called with passed attachmentId', () => {
  beforeEach(() => {
    vi.mocked(resolveAttachmentSrc).mockClear();
  });

  it('calls resolveAttachmentSrc with the provided attachmentId after mount', async () => {
    renderReader({ attachmentId: 'pdf-uuid-123' });
    await flushAsync();
    expect(resolveAttachmentSrc).toHaveBeenCalledWith('pdf-uuid-123');
  });

  it('calls resolveAttachmentSrc exactly once per mount', async () => {
    renderReader({ attachmentId: 'pdf-uuid-once' });
    await flushAsync();
    expect(resolveAttachmentSrc).toHaveBeenCalledTimes(1);
  });
});

// ─── PDF-4: <object data> receives the resolved URL ──────────────────────────

describe('PDF-4 — <object data> receives the resolved asset:// URL from the mock', () => {
  beforeEach(() => {
    vi.mocked(resolveAttachmentSrc).mockClear();
  });

  it('binds resolved URL to <object data> after onMount resolves', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValueOnce('asset://localhost/attachments/pdf-uuid-456.pdf');
    const { container } = renderReader({ attachmentId: 'pdf-uuid-456' });
    await flushAsync();

    const obj = container.querySelector('object');
    expect(obj?.getAttribute('data')).toBe('asset://localhost/attachments/pdf-uuid-456.pdf');
  });

  it('<object data> value matches the mock return (not a hardcoded fixture)', async () => {
    const mockUrl = 'asset://localhost/attachments/test-pdf-001.pdf';
    // Default mock returns asset://localhost/attachments/${id}.pdf — confirm flow-through.
    const { container } = renderReader({ attachmentId: 'test-pdf-001' });
    await flushAsync();

    const obj = container.querySelector('object');
    expect(obj?.getAttribute('data')).toBe(mockUrl);
  });
});

// ─── PDF-5: No base64 ────────────────────────────────────────────────────────

describe('PDF-5 — no base64: <object data> starts with asset://, never data:', () => {
  it('<object data> after resolution starts with asset://, not data:', async () => {
    const { container } = renderReader({ attachmentId: 'no-b64-pdf' });
    await flushAsync();

    const obj = container.querySelector('object');
    const data = obj?.getAttribute('data') ?? '';
    expect(data.startsWith('data:')).toBe(false);
    expect(data).toMatch(/^asset:\/\//);
  });

  it('<object data> never contains base64 payload', async () => {
    const { container } = renderReader({ attachmentId: 'no-b64-pdf-2' });
    await flushAsync();

    const obj = container.querySelector('object');
    const data = obj?.getAttribute('data') ?? '';
    expect(data).not.toMatch(/^data:/);
    expect(data).not.toMatch(/;base64,/);
  });
});

// ─── PDF-6: Empty resolve → no <object> rendered ─────────────────────────────

describe('PDF-6 — empty resolve → no <object> rendered (placeholder shown)', () => {
  it('does not render <object> when resolveAttachmentSrc returns empty string', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValueOnce('');
    const { container } = renderReader({ attachmentId: 'empty-resolve' });
    await flushAsync();

    const obj = container.querySelector('object');
    expect(obj).toBeNull();
  });

  it('renders the placeholder element when src is empty', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValueOnce('');
    const { container } = renderReader({ attachmentId: 'empty-placeholder' });
    await flushAsync();

    const placeholder = container.querySelector('.at-pdf-placeholder');
    expect(placeholder).not.toBeNull();
  });
});
