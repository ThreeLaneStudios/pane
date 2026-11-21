/**
 * AttachmentCard.og-enrich.test.ts — og: enrichment rendering for link cards (#1780)
 *
 * Tests:
 *  OG-1  og:title renders as the card name when ogTitle present
 *  OG-2  og:description renders as the meta line when ogDescription present
 *  OG-3  og:image thumbnail renders when ogImage present
 *  OG-4  Falls back to extractDomain(src) when all og fields null (regression guard)
 *  OG-5  Falls back to filename as name when ogTitle null
 *  OG-6  og:description overrides domain stub in meta line
 *  OG-7  og:image absent → no <img> element rendered (icon SVG instead)
 *
 * spec-covered-by: docs/spec/link-preview-og.md
 * Relates to #1780, ADR-0049 Decision 2
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import AttachmentCard from './AttachmentCard.svelte';

// ─── Helpers ─────────────────────────────────────────────────────────────────

function mkLinkAttachment(overrides: Record<string, unknown> = {}) {
  return {
    attachmentId: null,
    attachmentType: 'link',
    filename: null,
    src: 'https://example.com/article',
    alt: null,
    duration: null,
    pageCount: null,
    width: null,
    height: null,
    size: null,
    status: 'ready' as const,
    errorDetail: null,
    ogTitle: null,
    ogDescription: null,
    ogImage: null,
    ...overrides,
  };
}

// ─── OG-1: og:title renders as card name ─────────────────────────────────────

describe('AttachmentCard — og:title', () => {
  it('OG-1: renders og:title as the card name when ogTitle present', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({ ogTitle: 'A Great Article About Pane' }),
      },
    });
    const nameEl = container.querySelector('.at-name');
    expect(nameEl).not.toBeNull();
    expect(nameEl!.textContent).toBe('A Great Article About Pane');
  });

  it('OG-5: falls back to filename as name when ogTitle null', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({ filename: 'example.com', ogTitle: null }),
      },
    });
    const nameEl = container.querySelector('.at-name');
    expect(nameEl).not.toBeNull();
    expect(nameEl!.textContent).toBe('example.com');
  });
});

// ─── OG-2: og:description renders as meta line ───────────────────────────────

describe('AttachmentCard — og:description', () => {
  it('OG-2: renders og:description as meta line when ogDescription present', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          ogDescription: 'A short description of the article.',
        }),
      },
    });
    const metaEl = container.querySelector('.at-meta');
    expect(metaEl).not.toBeNull();
    expect(metaEl!.textContent).toBe('A short description of the article.');
  });

  it('OG-6: og:description overrides domain stub in meta line', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          src: 'https://example.com/article',
          ogDescription: 'og description wins',
        }),
      },
    });
    const metaEl = container.querySelector('.at-meta');
    expect(metaEl).not.toBeNull();
    // og:description should be shown, not the domain.
    expect(metaEl!.textContent).toBe('og description wins');
    expect(metaEl!.textContent).not.toContain('example.com');
  });
});

// ─── OG-3: remote image egress is blocked ───────────────────────────────────

describe('AttachmentCard — og:image egress', () => {
  it('OG-3: does not load a remote og:image URL in the WebView', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          ogImage: 'https://attacker.example/tracker.jpg?secret=value',
        }),
      },
    });
    expect(container.querySelector('.at-og-thumb')).toBeNull();
    expect(container.querySelector('img[src^="https://attacker.example"]')).toBeNull();
    expect(container.querySelector('.at-ico svg')).not.toBeNull();
  });

  it('OG-7: renders the local icon when ogImage is null', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({ ogImage: null }),
      },
    });
    expect(container.querySelector('.at-og-thumb')).toBeNull();
    expect(container.querySelector('.at-ico svg')).not.toBeNull();
  });
});

// ─── OG-4: Fallback to domain stub ───────────────────────────────────────────

describe('AttachmentCard — og: fallback to domain stub', () => {
  it('OG-4: falls back to extractDomain(src) in meta when all og fields null (regression guard)', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          src: 'https://www.example.com/page',
          ogTitle: null,
          ogDescription: null,
          ogImage: null,
        }),
      },
    });
    const metaEl = container.querySelector('.at-meta');
    expect(metaEl).not.toBeNull();
    // extractDomain strips www. and returns hostname.
    expect(metaEl!.textContent).toBe('example.com');
  });

  it('OG-4b: no .at-og-thumb rendered when ogImage null (domain stub path)', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          src: 'https://example.com/',
          ogImage: null,
        }),
      },
    });
    expect(container.querySelector('.at-og-thumb')).toBeNull();
  });
});
