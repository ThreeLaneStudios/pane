/**
 * resolveEmbed.test.ts — ADR-0050 §2 + Risks §90
 *
 * Test buckets:
 *   1. Look-alike hosts — all must return null
 *   2. Malformed IDs — all must return null
 *   3. Valid YouTube variants — canonical youtube-nocookie URL
 *   4. Valid Vimeo variants — canonical player.vimeo.com URL with ?dnt=1
 *   5. Query-injection defense — no user params in reconstructed canonical
 */

import { describe, it, expect } from 'vitest';
import { resolveEmbed, providerThumbnail } from './resolveEmbed';

// ─── 1. Look-alike hosts (ADR Risks §90) ─────────────────────────────────────

describe('resolveEmbed — look-alike hosts (all must return null)', () => {
  it('rejects youtube-nocookie.com.evil.tld', () => {
    expect(resolveEmbed('https://youtube-nocookie.com.evil.tld/embed/dQw4w9WgXcQ')).toBeNull();
  });

  it('rejects vimeo.com.attacker.tld', () => {
    expect(resolveEmbed('https://vimeo.com.attacker.tld/123456789')).toBeNull();
  });

  it('rejects www.youtube.com.evil.tld', () => {
    expect(resolveEmbed('https://www.youtube.com.evil.tld/watch?v=dQw4w9WgXcQ')).toBeNull();
  });

  it('rejects evil.tld/youtube.com/watch path', () => {
    expect(resolveEmbed('https://evil.tld/youtube.com/watch?v=dQw4w9WgXcQ')).toBeNull();
  });

  it('rejects evil.youtube.com (subdomain squat)', () => {
    expect(resolveEmbed('https://evil.youtube.com/watch?v=dQw4w9WgXcQ')).toBeNull();
  });

  it('rejects youtube.com.example.com', () => {
    expect(resolveEmbed('https://youtube.com.example.com/watch?v=dQw4w9WgXcQ')).toBeNull();
  });

  it('rejects subdomain of vimeo (evil.vimeo.com)', () => {
    expect(resolveEmbed('https://evil.vimeo.com/123456789')).toBeNull();
  });
});

// ─── 2. Malformed IDs ────────────────────────────────────────────────────────

describe('resolveEmbed — malformed IDs (all must return null)', () => {
  it('rejects YouTube id too short (< 11 chars)', () => {
    expect(resolveEmbed('https://youtube.com/watch?v=abc')).toBeNull();
  });

  it('rejects YouTube id too long (> 11 chars)', () => {
    expect(resolveEmbed('https://youtube.com/watch?v=abcdefghijklmnop')).toBeNull();
  });

  it('rejects YouTube id with invalid chars', () => {
    expect(resolveEmbed('https://youtube.com/watch?v=abc!@#$def123')).toBeNull();
  });

  it('rejects YouTube id with spaces', () => {
    expect(resolveEmbed('https://www.youtube.com/watch?v=dQw4w9WgX Q')).toBeNull();
  });

  it('rejects Vimeo id non-numeric', () => {
    expect(resolveEmbed('https://vimeo.com/abcxyz')).toBeNull();
  });

  it('rejects Vimeo id with mixed alphanumeric', () => {
    expect(resolveEmbed('https://vimeo.com/abc123def')).toBeNull();
  });

  it('rejects empty string', () => {
    expect(resolveEmbed('')).toBeNull();
  });

  it('rejects non-URL string', () => {
    expect(resolveEmbed('not a url at all')).toBeNull();
  });

  it('rejects Vimeo with empty path', () => {
    expect(resolveEmbed('https://vimeo.com/')).toBeNull();
  });

  it('rejects data: URL', () => {
    expect(resolveEmbed('data:text/html,<h1>x</h1>')).toBeNull();
  });

  it('rejects javascript: URL', () => {
    expect(resolveEmbed('javascript:alert(1)')).toBeNull();
  });

  it('rejects YouTube watch URL missing v param', () => {
    expect(resolveEmbed('https://www.youtube.com/watch')).toBeNull();
  });
});

// ─── 3. Valid YouTube variants ────────────────────────────────────────────────

const EXPECTED_YOUTUBE_CANONICAL = 'https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ';
const EXPECTED_YOUTUBE_ID = 'dQw4w9WgXcQ';

describe('resolveEmbed — valid YouTube variants', () => {
  it('resolves www.youtube.com/watch?v=', () => {
    const result = resolveEmbed('https://www.youtube.com/watch?v=dQw4w9WgXcQ');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('youtube');
    expect(result!.videoId).toBe(EXPECTED_YOUTUBE_ID);
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
  });

  it('resolves youtu.be/{id}', () => {
    const result = resolveEmbed('https://youtu.be/dQw4w9WgXcQ');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('youtube');
    expect(result!.videoId).toBe(EXPECTED_YOUTUBE_ID);
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
  });

  it('resolves m.youtube.com/watch?v=', () => {
    const result = resolveEmbed('https://m.youtube.com/watch?v=dQw4w9WgXcQ');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('youtube');
    expect(result!.videoId).toBe(EXPECTED_YOUTUBE_ID);
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
  });

  it('resolves youtube.com (no www)/watch?v=', () => {
    const result = resolveEmbed('https://youtube.com/watch?v=dQw4w9WgXcQ');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('youtube');
    expect(result!.videoId).toBe(EXPECTED_YOUTUBE_ID);
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
  });

  it('resolves www.youtube.com/embed/{id} (already-embed form)', () => {
    const result = resolveEmbed('https://www.youtube.com/embed/dQw4w9WgXcQ');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('youtube');
    expect(result!.videoId).toBe(EXPECTED_YOUTUBE_ID);
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
  });
});

// ─── 4. Valid Vimeo variants ──────────────────────────────────────────────────

describe('resolveEmbed — valid Vimeo variants', () => {
  it('resolves vimeo.com/{id}', () => {
    const result = resolveEmbed('https://vimeo.com/123456789');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('vimeo');
    expect(result!.videoId).toBe('123456789');
    expect(result!.canonicalUrl).toBe('https://player.vimeo.com/video/123456789?dnt=1');
  });

  it('resolves player.vimeo.com/video/{id}', () => {
    const result = resolveEmbed('https://player.vimeo.com/video/123456789');
    expect(result).not.toBeNull();
    expect(result!.provider).toBe('vimeo');
    expect(result!.videoId).toBe('123456789');
    expect(result!.canonicalUrl).toBe('https://player.vimeo.com/video/123456789?dnt=1');
  });
});

// ─── 5. Query-injection defense ───────────────────────────────────────────────

describe('resolveEmbed — query-injection defense', () => {
  it('strips extra query params from YouTube — canonical has no query', () => {
    const result = resolveEmbed('https://youtube.com/watch?v=dQw4w9WgXcQ&malicious=x&list=PLfoo');
    expect(result).not.toBeNull();
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
    // Must NOT contain any user-supplied params
    expect(result!.canonicalUrl).not.toContain('malicious');
    expect(result!.canonicalUrl).not.toContain('list');
  });

  it('Vimeo canonical is exactly ?dnt=1 — no extra params from user URL', () => {
    const result = resolveEmbed('https://vimeo.com/123456789?autoplay=1&evil=1');
    expect(result).not.toBeNull();
    expect(result!.canonicalUrl).toBe('https://player.vimeo.com/video/123456789?dnt=1');
    expect(result!.canonicalUrl).not.toContain('autoplay');
    expect(result!.canonicalUrl).not.toContain('evil');
  });

  it('youtu.be with query params returns clean canonical', () => {
    const result = resolveEmbed('https://youtu.be/dQw4w9WgXcQ?si=abc123&feature=shared');
    expect(result).not.toBeNull();
    expect(result!.canonicalUrl).toBe(EXPECTED_YOUTUBE_CANONICAL);
    expect(result!.canonicalUrl).not.toContain('si=');
    expect(result!.canonicalUrl).not.toContain('feature=');
  });
});

// ─── 6. providerThumbnail() ─────────────────────────────────────────────────

describe('providerThumbnail — YouTube CDN URL construction', () => {
  it('returns hqdefault.jpg CDN URL for YouTube provider', () => {
    const url = providerThumbnail('youtube', 'dQw4w9WgXcQ');
    expect(url).toBe('https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg');
  });

  it('interpolates arbitrary videoId into the CDN path', () => {
    const url = providerThumbnail('youtube', 'hOy-cX_2fQ0');
    expect(url).toBe('https://img.youtube.com/vi/hOy-cX_2fQ0/hqdefault.jpg');
  });
});

describe('providerThumbnail — Vimeo returns null (deferred)', () => {
  it('returns null for Vimeo (oEmbed API call required, deferred to follow-up)', () => {
    const url = providerThumbnail('vimeo', '123456789');
    expect(url).toBeNull();
  });
});
