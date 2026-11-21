/**
 * AttachmentNodeView.link-render.test.ts — #1898 link plain + preview render fix
 *
 * Verifies that link attachment nodes render their correct mode rather than
 * falling through to AttachmentCard's skeleton loader.
 *
 *  LPR-1  link + plain renders <a href={src}> with URL as visible text
 *  LPR-2  link + plain anchor has target="_blank" + rel="noopener noreferrer"
 *  LPR-3  link + plain does NOT render .at-skel, .is-loading, .at-card, or .at-spin
 *  LPR-4  link + preview renders AttachmentCard component (assert on .at-card presence)
 *  LPR-5  link + preview renders the domain in the card meta (extractDomain output)
 *  LPR-6  link + preview does NOT render .at-skel or .is-loading
 *  LPR-7  [REMOVED — AttachmentLinkEmbed deleted in #1903; replaced by #1902 video card]
 *  LPR-8  regression: file + card still hits AttachmentCard else fallthrough (.at-card)
 *  LPR-9  link + preview + allowlisted URL → AttachmentVideoCard renders (thumbnail img, provider label, "Open in YouTube" button)
 *  LPR-10 link + preview + non-allowlisted URL → plain AttachmentCard renders (regression: domain stub unchanged)
 *
 * spec-skip: pure UI fix; no core-behavioral files touched; adds missing link
 *            plain/preview render branches per LOCKED-DESIGN-SPEC.md (Reed audit
 *            2026-07-04-bug1-link-plain-skeleton.md); fixes isLoading heuristic
 *            to exclude link attachments
 * Relates to #1898
 */

import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import AttachmentNodeView from './AttachmentNodeView.svelte';

// ─── Mocks ───────────────────────────────────────────────────────────────────

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn().mockResolvedValue(''),
  openAttachmentExternally: vi.fn().mockResolvedValue(undefined),
  // og-fetch (#1780): return null by default (no og data) so existing tests are unaffected.
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
}));

vi.mock('$lib/stores/player', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    loadTrack: vi.fn(),
  };
});

// ─── Helpers ──────────────────────────────────────────────────────────────────

function mkLinkAttrs(overrides: Record<string, unknown> = {}) {
  return {
    attachmentId: null,
    attachmentType: 'link',
    filename: null,
    src: 'https://www.youtube.com/watch?v=hOy-cX_2fQ0',
    alt: null,
    duration: null,
    pageCount: null,
    width: null,
    height: null,
    size: null,
    status: null,
    errorDetail: null,
    renderMode: 'plain',
    ...overrides,
  };
}

function mkFileAttrs(overrides: Record<string, unknown> = {}) {
  return {
    attachmentId: 'att-uuid-abc123',
    attachmentType: 'file',
    filename: 'report.pdf',
    src: null,
    alt: null,
    duration: null,
    pageCount: null,
    width: null,
    height: null,
    size: 204800,
    status: 'ready' as const,
    errorDetail: null,
    renderMode: 'card',
    ...overrides,
  };
}

// ─── LPR-1: link + plain renders <a href={src}> with URL as text ─────────────

describe('LPR-1: link + plain renders anchor with URL as text', () => {
  it('renders an anchor element with href matching the src', () => {
    const src = 'https://www.youtube.com/watch?v=hOy-cX_2fQ0';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'plain' }) },
    });

    const anchor = container.querySelector('a.at-link-plain');
    expect(anchor).not.toBeNull();
    expect(anchor!.getAttribute('href')).toBe(src);
    expect(anchor!.textContent?.trim()).toBe(src);
  });
});

// ─── LPR-2: link + plain anchor has target + rel ─────────────────────────────

describe('LPR-2: link + plain anchor has target="_blank" + rel="noopener noreferrer"', () => {
  it('has target="_blank"', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    const anchor = container.querySelector('a.at-link-plain');
    expect(anchor).not.toBeNull();
    expect(anchor!.getAttribute('target')).toBe('_blank');
  });

  it('has rel="noopener noreferrer"', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    const anchor = container.querySelector('a.at-link-plain');
    expect(anchor).not.toBeNull();
    expect(anchor!.getAttribute('rel')).toBe('noopener noreferrer');
  });
});

// ─── LPR-3: link + plain does NOT render skeleton ─────────────────────────────

describe('LPR-3: link + plain does not render loading skeleton', () => {
  it('does not render .at-skel anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    expect(container.querySelector('.at-skel')).toBeNull();
  });

  it('does not render .is-loading anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    expect(container.querySelector('.is-loading')).toBeNull();
  });

  it('does not render .at-card anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    expect(container.querySelector('.at-card')).toBeNull();
  });

  it('does not render .at-spin anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'plain' }) },
    });

    expect(container.querySelector('.at-spin')).toBeNull();
  });
});

// ─── LPR-4: link + preview + non-allowlisted URL renders AttachmentCard ────────
// Note: allowlisted URLs (YouTube, Vimeo) now render AttachmentVideoCard
// (see LPR-9). This test covers the non-allowlisted fallthrough path.

describe('LPR-4: link + preview (non-allowlisted) renders AttachmentCard', () => {
  it('renders .at-card element for a non-allowlisted URL', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src: 'https://pane.so', renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-card')).not.toBeNull();
  });
});

// ─── LPR-5: link + preview shows domain in card meta (non-allowlisted) ────────
// YouTube URLs now render AttachmentVideoCard (not AttachmentCard).
// The domain-in-meta assertion only applies to the non-allowlisted fallthrough.

describe('LPR-5: link + preview renders domain in card meta (non-allowlisted)', () => {
  it('shows github.com for a github URL (non-allowlisted domain)', () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkLinkAttrs({
          src: 'https://github.com/some/repo',
          renderMode: 'preview',
        }),
      },
    });

    const metaEl = container.querySelector('.at-meta');
    expect(metaEl).not.toBeNull();
    expect(metaEl!.textContent?.trim()).toBe('github.com');
  });

  it('shows pane.so for a pane.so URL', () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkLinkAttrs({
          src: 'https://pane.so',
          renderMode: 'preview',
        }),
      },
    });

    const metaEl = container.querySelector('.at-meta');
    expect(metaEl).not.toBeNull();
    expect(metaEl!.textContent?.trim()).toBe('pane.so');
  });
});

// ─── LPR-6: link + preview does NOT render skeleton ──────────────────────────

describe('LPR-6: link + preview does not render loading skeleton', () => {
  it('does not render .at-skel anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-skel')).toBeNull();
  });

  it('does not render .is-loading anywhere', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ renderMode: 'preview' }) },
    });

    expect(container.querySelector('.is-loading')).toBeNull();
  });
});

// ─── LPR-8: regression — file + card still hits else fallthrough (.at-card) ──

describe('LPR-8: regression — file + card still renders AttachmentCard via else fallthrough', () => {
  it('renders .at-card for a file attachment in card mode', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkFileAttrs({ renderMode: 'card' }) },
    });

    expect(container.querySelector('.at-card')).not.toBeNull();
  });

  it('renders file icon (svg in .at-ico) for file type', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkFileAttrs({ attachmentType: 'file', renderMode: 'card' }) },
    });

    const ico = container.querySelector('.at-ico svg');
    expect(ico).not.toBeNull();
  });
});

// ─── LPR-9: link + preview + allowlisted URL → AttachmentVideoCard renders ───

describe('LPR-9: link + preview + allowlisted URL renders AttachmentVideoCard', () => {
  it('renders thumbnail img with YouTube CDN src for a YouTube URL', () => {
    const src = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    const img = container.querySelector('img.at-thumb-img');
    expect(img).not.toBeNull();
    expect(img!.getAttribute('src')).toBe(
      'https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg'
    );
  });

  it('renders provider label "YouTube" in .at-meta', () => {
    const src = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    const meta = container.querySelector('.at-meta');
    expect(meta).not.toBeNull();
    expect(meta!.textContent?.trim()).toBe('YouTube');
  });

  it('renders "Open in YouTube" button', () => {
    const src = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    const btn = container.querySelector('button.at-open-btn');
    expect(btn).not.toBeNull();
    expect(btn!.textContent?.trim()).toContain('Open in YouTube');
  });

  it('does NOT render .at-card (plain AttachmentCard) for allowlisted URL', () => {
    const src = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    // AttachmentCard uses .at-card; AttachmentVideoCard uses .at-video-card
    expect(container.querySelector('.at-card')).toBeNull();
    expect(container.querySelector('.at-video-card')).not.toBeNull();
  });
});

// ─── LPR-10: link + preview + non-allowlisted URL → plain AttachmentCard ────

describe('LPR-10: link + preview + non-allowlisted URL renders plain AttachmentCard (regression)', () => {
  it('renders .at-card for a non-allowlisted domain', () => {
    const src = 'https://pane.so';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-card')).not.toBeNull();
  });

  it('does NOT render .at-video-card for a non-allowlisted domain', () => {
    const src = 'https://pane.so';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-video-card')).toBeNull();
  });

  it('renders domain in .at-meta for non-allowlisted fallthrough', () => {
    const src = 'https://pane.so';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    const meta = container.querySelector('.at-meta');
    expect(meta).not.toBeNull();
    expect(meta!.textContent?.trim()).toBe('pane.so');
  });
});

// ─── LPR-11: link + preview + og data present → enriched card ────────────────

describe('LPR-11: link + preview + og data → enriched AttachmentCard', () => {
  it('renders .at-card for a non-allowlisted domain with og data', async () => {
    const src = 'https://pane.app/blog/post';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    // AttachmentCard should render (not video card).
    expect(container.querySelector('.at-card')).not.toBeNull();
    expect(container.querySelector('.at-video-card')).toBeNull();
  });

  it('does NOT break video card for allowlisted URL when og mocks are present', () => {
    // Regression: og fetch mocks must not interfere with the video card path.
    const src = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-video-card')).not.toBeNull();
    expect(container.querySelector('.at-card')).toBeNull();
  });
});

// ─── LPR-12: link + preview + og null → domain stub fallback ─────────────────

describe('LPR-12: link + preview + no og data → domain stub fallback', () => {
  it('renders domain stub when getCachedLinkPreview returns null', () => {
    // fetchLinkPreview and getCachedLinkPreview both mocked to return null above.
    const src = 'https://fallback-domain.example.com/page';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    const meta = container.querySelector('.at-meta');
    expect(meta).not.toBeNull();
    // extractDomain should show the hostname.
    expect(meta!.textContent?.trim()).toBe('fallback-domain.example.com');
  });

  it('renders .at-card (not video card) when og null and domain is non-allowlisted', () => {
    const src = 'https://fallback-domain.example.com/page';
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkLinkAttrs({ src, renderMode: 'preview' }) },
    });

    expect(container.querySelector('.at-card')).not.toBeNull();
    expect(container.querySelector('.at-video-card')).toBeNull();
  });
});
