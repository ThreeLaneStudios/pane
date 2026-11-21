/**
 * AttachmentVideoPlayer.test.ts — PR4 (#401)
 *
 * Tests for the inline video player component.
 *
 * Tests:
 *  V-1  Mount → 16:9 aspect-ratio container (.at-video-ratio) present
 *  V-2  <video controls> present
 *  V-3  autoplay attribute absent (Svelte removes attr when autoplay={false})
 *  V-4  Src resolution: resolveAttachmentSrc called; URL bound to <video src>
 *  V-5  No base64: <video src> starts with asset://, never data:
 *  V-6  Poster: not set to anything bogus; no canvas extraction
 *  V-7  Duration display: shown when set; controls render even if unset
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AttachmentVideoPlayer from './AttachmentVideoPlayer.svelte';

// ─── Mock resolveAttachmentSrc ────────────────────────────────────────────────
vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn(async (id: string) => `asset://localhost/attachments/${id}.mp4`),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
}));

import { resolveAttachmentSrc } from '$lib/api';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function renderPlayer(overrides: Record<string, any> = {}) {
  return render(AttachmentVideoPlayer, {
    props: {
      attachmentId: 'test-video-001',
      filename: 'clip.mp4',
      duration: null,
      ...overrides,
    },
  });
}

// Flush async (onMount resolveAttachmentSrc) + Svelte 5 state update.
async function flushAsync() {
  await Promise.resolve(); // drain microtask queue
  await tick();            // flush Svelte reactive state
}

// ─── V-1: 16:9 aspect-ratio container ────────────────────────────────────────

describe('V-1 — 16:9 aspect-ratio container present', () => {
  it('renders the .at-video-ratio container', () => {
    const { container } = renderPlayer();
    const ratio = container.querySelector('.at-video-ratio');
    expect(ratio).not.toBeNull();
  });

  it('.at-video-player outer wrapper is rendered', () => {
    const { container } = renderPlayer();
    expect(container.querySelector('.at-video-player')).not.toBeNull();
  });
});

// ─── V-2: <video controls> present ───────────────────────────────────────────

describe('V-2 — <video controls> present', () => {
  it('renders a <video> element', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video');
    expect(video).not.toBeNull();
  });

  it('<video> has controls attribute', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video');
    expect(video).not.toBeNull();
    expect(video?.hasAttribute('controls')).toBe(true);
  });
});

// ─── V-3: autoplay absent / false ────────────────────────────────────────────

describe('V-3 — autoplay attribute absent or false', () => {
  it('<video> autoplay is false (Svelte removes boolean attr when false)', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video') as HTMLVideoElement | null;
    expect(video).not.toBeNull();
    // Svelte 5: autoplay={false} → no autoplay attribute set / autoplay property false
    expect(video?.autoplay).toBe(false);
  });

  it('<video> does not have autoplay="true" attribute', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video');
    expect(video?.getAttribute('autoplay')).not.toBe('true');
  });
});

// ─── V-4: Src resolution ─────────────────────────────────────────────────────

describe('V-4 — src resolution: resolveAttachmentSrc called; URL bound to <video src>', () => {
  beforeEach(() => {
    vi.mocked(resolveAttachmentSrc).mockClear();
  });

  it('calls resolveAttachmentSrc with the provided attachmentId after mount', async () => {
    renderPlayer({ attachmentId: 'video-uuid-123' });
    await flushAsync();
    expect(resolveAttachmentSrc).toHaveBeenCalledWith('video-uuid-123');
  });

  it('binds resolved URL to <video src> after onMount resolves', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValueOnce('asset://localhost/attachments/video-uuid-456.mp4');
    const { container } = renderPlayer({ attachmentId: 'video-uuid-456' });
    await flushAsync();

    const video = container.querySelector('video');
    expect(video?.getAttribute('src')).toBe('asset://localhost/attachments/video-uuid-456.mp4');
  });

  it('<video> element is in the DOM before src resolves (not hidden)', () => {
    // Test that video is always in DOM (not conditional on src)
    const { container } = renderPlayer({ attachmentId: 'early-render-test' });
    const video = container.querySelector('video');
    expect(video).not.toBeNull();
  });
});

// ─── V-5: No base64 ──────────────────────────────────────────────────────────

describe('V-5 — no base64: <video src> starts with asset://, never data:', () => {
  it('<video src> after resolution starts with asset://, not data:', async () => {
    const { container } = renderPlayer({ attachmentId: 'no-b64-video' });
    await flushAsync();

    const video = container.querySelector('video');
    const src = video?.getAttribute('src') ?? '';
    expect(src.startsWith('data:')).toBe(false);
    expect(src).toMatch(/^asset:\/\//);
  });

  it('<video src> never contains base64 payload', async () => {
    const { container } = renderPlayer({ attachmentId: 'no-b64-video-2' });
    await flushAsync();

    const video = container.querySelector('video');
    const src = video?.getAttribute('src') ?? '';
    expect(src).not.toMatch(/^data:/);
    expect(src).not.toMatch(/;base64,/);
  });
});

// ─── V-6: Poster — not set to bogus value ─────────────────────────────────────

describe('V-6 — poster: not set to anything bogus', () => {
  it('<video> poster (if present) is not a data: URI', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video');
    const poster = video?.getAttribute('poster');
    // We never set a bogus poster: absent OR a legitimate URL
    if (poster !== null && poster !== undefined && poster !== '') {
      expect(poster).not.toMatch(/^data:/);
    }
    // We do NOT assert poster is absent — browser may provide natural first-frame.
    // We DO assert we never set it to a data: URI (no canvas extraction).
  });

  it('component does not create a <canvas> element (no poster extraction)', () => {
    // Verify no canvas element was created (no poster extraction logic).
    const { container } = renderPlayer();
    const canvas = container.querySelector('canvas');
    expect(canvas).toBeNull();
  });

  it('preload="metadata" attribute present (allows browser first-frame if supported)', () => {
    const { container } = renderPlayer();
    const video = container.querySelector('video');
    // We use preload=metadata so the browser can provide a natural first frame.
    expect(video?.getAttribute('preload')).toBe('metadata');
  });
});

// ─── V-7: Duration display ────────────────────────────────────────────────────

describe('V-7 — duration display', () => {
  it('shows duration when attrs.duration is set (93s = 1:33)', () => {
    const { container } = renderPlayer({ duration: 93 });

    const meta = container.querySelector('.at-video-meta');
    expect(meta).not.toBeNull();

    const durationEl = container.querySelector('.at-video-duration');
    expect(durationEl).not.toBeNull();
    expect(durationEl?.textContent).toContain('1:33');
  });

  it('<video controls> still renders when duration is null', () => {
    const { container } = renderPlayer({ duration: null });
    const video = container.querySelector('video');
    expect(video).not.toBeNull();
    expect(video?.hasAttribute('controls')).toBe(true);
    // Duration meta absent when null
    const meta = container.querySelector('.at-video-meta');
    expect(meta).toBeNull();
  });

  it('formats long video duration correctly (3723s = 1:02:03)', () => {
    const { container } = renderPlayer({ duration: 3723 });
    const durationEl = container.querySelector('.at-video-duration');
    expect(durationEl).not.toBeNull();
    expect(durationEl?.textContent).toContain('1:02:03');
  });

  it('formats short duration correctly (90s = 1:30)', () => {
    const { container } = renderPlayer({ duration: 90 });
    const durationEl = container.querySelector('.at-video-duration');
    expect(durationEl?.textContent).toContain('1:30');
  });
});
