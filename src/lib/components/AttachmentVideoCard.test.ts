/**
 * AttachmentVideoCard.test.ts — #1902 YouTube video card component
 *
 * Covers:
 *   AVC-1  YouTube: renders thumbnail img with correct CDN src
 *   AVC-2  YouTube: renders "YouTube" provider label in .at-meta
 *   AVC-3  YouTube: renders "Open in YouTube" button
 *   AVC-4  YouTube: button click calls open() with correct watch URL
 *   AVC-5  Vimeo: renders play-icon placeholder (no img — thumbnail deferred)
 *   AVC-6  Vimeo: renders "Vimeo" provider label in .at-meta
 *   AVC-7  Vimeo: renders "Open in Vimeo" button
 *   AVC-8  Vimeo: button click calls open() with correct Vimeo URL
 *   AVC-9  filename prop: shown as title when present
 *   AVC-10 filename absent: falls back to src URL as title
 *
 * Relates to #1902
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import AttachmentVideoCard from './AttachmentVideoCard.svelte';

// ─── Mocks ───────────────────────────────────────────────────────────────────

// vi.hoisted is required because vi.mock() is hoisted before variable declarations.
const mockOpen = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: mockOpen,
}));

beforeEach(() => {
  mockOpen.mockClear();
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

const YOUTUBE_VIDEO_ID = 'dQw4w9WgXcQ';
const VIMEO_VIDEO_ID = '123456789';

function mkYouTubeProps(overrides: Record<string, unknown> = {}) {
  return {
    provider: 'youtube' as const,
    videoId: YOUTUBE_VIDEO_ID,
    filename: 'Rick Astley - Never Gonna Give You Up',
    src: `https://www.youtube.com/watch?v=${YOUTUBE_VIDEO_ID}`,
    ...overrides,
  };
}

function mkVimeoProps(overrides: Record<string, unknown> = {}) {
  return {
    provider: 'vimeo' as const,
    videoId: VIMEO_VIDEO_ID,
    filename: 'My Vimeo Video',
    src: `https://vimeo.com/${VIMEO_VIDEO_ID}`,
    ...overrides,
  };
}

// ─── AVC-1: YouTube thumbnail img ────────────────────────────────────────────

describe('AVC-1: YouTube renders thumbnail img with CDN src', () => {
  it('renders img.at-thumb-img with YouTube CDN URL', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const img = container.querySelector('img.at-thumb-img');
    expect(img).not.toBeNull();
    expect(img!.getAttribute('src')).toBe(
      `https://img.youtube.com/vi/${YOUTUBE_VIDEO_ID}/hqdefault.jpg`
    );
  });

  it('img has empty alt (decorative)', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const img = container.querySelector('img.at-thumb-img');
    expect(img!.getAttribute('alt')).toBe('');
  });
});

// ─── AVC-2: YouTube provider label ───────────────────────────────────────────

describe('AVC-2: YouTube renders "YouTube" provider label', () => {
  it('renders .at-meta with text "YouTube"', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const meta = container.querySelector('.at-meta');
    expect(meta).not.toBeNull();
    expect(meta!.textContent?.trim()).toBe('YouTube');
  });
});

// ─── AVC-3: YouTube "Open in YouTube" button ─────────────────────────────────

describe('AVC-3: YouTube renders "Open in YouTube" button', () => {
  it('renders button.at-open-btn', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const btn = container.querySelector('button.at-open-btn');
    expect(btn).not.toBeNull();
  });

  it('button text contains "Open in YouTube"', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const btn = container.querySelector('button.at-open-btn');
    expect(btn!.textContent?.trim()).toContain('Open in YouTube');
  });

  it('button aria-label is "Open in YouTube"', () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const btn = container.querySelector('button.at-open-btn');
    expect(btn!.getAttribute('aria-label')).toBe('Open in YouTube');
  });
});

// ─── AVC-4: YouTube button click calls open() ────────────────────────────────

describe('AVC-4: YouTube button click calls open() with watch URL', () => {
  it('calls open() with https://www.youtube.com/watch?v={id} on click', async () => {
    const { container } = render(AttachmentVideoCard, { props: mkYouTubeProps() });

    const btn = container.querySelector('button.at-open-btn')!;
    await fireEvent.click(btn);

    expect(mockOpen).toHaveBeenCalledOnce();
    expect(mockOpen).toHaveBeenCalledWith(
      `https://www.youtube.com/watch?v=${YOUTUBE_VIDEO_ID}`
    );
  });
});

// ─── AVC-5: Vimeo placeholder (no img) ───────────────────────────────────────

describe('AVC-5: Vimeo renders play-icon placeholder, not img', () => {
  it('does NOT render img.at-thumb-img for Vimeo', () => {
    const { container } = render(AttachmentVideoCard, { props: mkVimeoProps() });

    expect(container.querySelector('img.at-thumb-img')).toBeNull();
  });

  it('renders svg play icon in .at-thumb', () => {
    const { container } = render(AttachmentVideoCard, { props: mkVimeoProps() });

    const thumb = container.querySelector('.at-thumb');
    expect(thumb).not.toBeNull();
    expect(thumb!.querySelector('svg')).not.toBeNull();
  });
});

// ─── AVC-6: Vimeo provider label ─────────────────────────────────────────────

describe('AVC-6: Vimeo renders "Vimeo" provider label', () => {
  it('renders .at-meta with text "Vimeo"', () => {
    const { container } = render(AttachmentVideoCard, { props: mkVimeoProps() });

    const meta = container.querySelector('.at-meta');
    expect(meta).not.toBeNull();
    expect(meta!.textContent?.trim()).toBe('Vimeo');
  });
});

// ─── AVC-7: Vimeo "Open in Vimeo" button ─────────────────────────────────────

describe('AVC-7: Vimeo renders "Open in Vimeo" button', () => {
  it('button text contains "Open in Vimeo"', () => {
    const { container } = render(AttachmentVideoCard, { props: mkVimeoProps() });

    const btn = container.querySelector('button.at-open-btn');
    expect(btn).not.toBeNull();
    expect(btn!.textContent?.trim()).toContain('Open in Vimeo');
  });
});

// ─── AVC-8: Vimeo button click calls open() ──────────────────────────────────

describe('AVC-8: Vimeo button click calls open() with Vimeo URL', async () => {
  it('calls open() with https://vimeo.com/{id} on click', async () => {
    const { container } = render(AttachmentVideoCard, { props: mkVimeoProps() });

    const btn = container.querySelector('button.at-open-btn')!;
    await fireEvent.click(btn);

    expect(mockOpen).toHaveBeenCalledOnce();
    expect(mockOpen).toHaveBeenCalledWith(`https://vimeo.com/${VIMEO_VIDEO_ID}`);
  });
});

// ─── AVC-9: filename prop shown as title ─────────────────────────────────────

describe('AVC-9: filename prop shown as title in .at-name', () => {
  it('renders filename text in .at-name', () => {
    const { container } = render(AttachmentVideoCard, {
      props: mkYouTubeProps({ filename: 'My Custom Title' }),
    });

    const name = container.querySelector('.at-name');
    expect(name).not.toBeNull();
    expect(name!.textContent?.trim()).toBe('My Custom Title');
  });
});

// ─── AVC-10: filename absent → fallback to src URL ───────────────────────────

describe('AVC-10: filename absent falls back to src URL as title', () => {
  it('shows src URL in .at-name when filename is null', () => {
    const src = `https://www.youtube.com/watch?v=${YOUTUBE_VIDEO_ID}`;
    const { container } = render(AttachmentVideoCard, {
      props: mkYouTubeProps({ filename: null, src }),
    });

    const name = container.querySelector('.at-name');
    expect(name).not.toBeNull();
    expect(name!.textContent?.trim()).toBe(src);
  });
});
