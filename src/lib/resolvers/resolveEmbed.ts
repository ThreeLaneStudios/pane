// Retained for #1902 thumbnail card — see audit 2026-07-04-embed-rip-scope.md

/**
 * resolveEmbed.ts — Provider allowlist + canonical URL reconstruction (ADR-0050 §2).
 *
 * Parses a user-supplied URL, matches it against the allowlisted provider set,
 * extracts a video id using strict patterns, and reconstructs the canonical
 * embed URL from a trusted template. Never passes the raw user URL through.
 *
 * Returns null on:
 *   - Invalid URL (URL constructor throws)
 *   - Non-allowlisted host (exact registrable-domain match — no substring)
 *   - Missing or malformed video id
 *   - Empty or non-string input
 *
 * Security: host matching uses exact equality against the allowlist set. The
 * look-alike attack (youtube-nocookie.com.evil.tld) is defeated because
 * URL().hostname returns the full host and we check exact membership.
 *
 * ADR-0050 §2, Risks §90.
 */

export interface ResolvedEmbed {
  provider: 'youtube' | 'vimeo';
  canonicalUrl: string;
  videoId: string;
}

// ─── Allowlist sets (exact hostname match only) ───────────────────────────────

const YOUTUBE_HOSTS = new Set([
  'youtube.com',
  'www.youtube.com',
  'm.youtube.com',
  'youtu.be',
]);

const VIMEO_HOSTS = new Set([
  'vimeo.com',
  'player.vimeo.com',
]);

// ─── ID patterns ─────────────────────────────────────────────────────────────

/** YouTube video IDs: exactly 11 chars from [A-Za-z0-9_-] */
const YOUTUBE_ID_RE = /^[A-Za-z0-9_-]{11}$/;

/** Vimeo video IDs: numeric only, at least 1 digit */
const VIMEO_ID_RE = /^[0-9]+$/;

// ─── Resolver ────────────────────────────────────────────────────────────────

export function resolveEmbed(url: string): ResolvedEmbed | null {
  if (!url || typeof url !== 'string') return null;

  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    // Invalid URL — do NOT fall back to substring matching
    return null;
  }

  // Only http(s) schemes — block data:, blob:, javascript:, etc.
  if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') return null;

  const host = parsed.hostname.toLowerCase();

  // ─── YouTube ─────────────────────────────────────────────────────────────
  if (YOUTUBE_HOSTS.has(host)) {
    let videoId: string | null = null;

    if (host === 'youtu.be') {
      // https://youtu.be/{id}
      const parts = parsed.pathname.split('/').filter(Boolean);
      videoId = parts[0] ?? null;
    } else {
      // https://www.youtube.com/watch?v={id}
      const v = parsed.searchParams.get('v');
      if (v) {
        videoId = v;
      } else {
        // https://www.youtube.com/embed/{id}
        const embedMatch = parsed.pathname.match(/^\/embed\/([^/?#]+)/);
        videoId = embedMatch ? (embedMatch[1] ?? null) : null;
      }
    }

    if (!videoId || !YOUTUBE_ID_RE.test(videoId)) return null;

    // Reconstruct canonical URL from trusted template — never passthrough
    return {
      provider: 'youtube',
      videoId,
      canonicalUrl: `https://www.youtube-nocookie.com/embed/${videoId}`,
    };
  }

  // ─── Vimeo ───────────────────────────────────────────────────────────────
  if (VIMEO_HOSTS.has(host)) {
    let videoId: string | null = null;

    if (host === 'player.vimeo.com') {
      // https://player.vimeo.com/video/{id}
      const match = parsed.pathname.match(/^\/video\/([^/?#]+)/);
      videoId = match ? (match[1] ?? null) : null;
    } else {
      // https://vimeo.com/{id}
      const parts = parsed.pathname.split('/').filter(Boolean);
      videoId = parts[0] ?? null;
    }

    if (!videoId || !VIMEO_ID_RE.test(videoId)) return null;

    // Reconstruct canonical URL from trusted template — never passthrough
    return {
      provider: 'vimeo',
      videoId,
      canonicalUrl: `https://player.vimeo.com/video/${videoId}?dnt=1`,
    };
  }

  // Non-allowlisted host
  return null;
}

/**
 * Build a static thumbnail URL for an allowlisted provider + videoId.
 *
 * YouTube: hqdefault.jpg from public CDN — no auth, no network call at
 * construction time. CSP img-src must include https://img.youtube.com
 * (added in #1902 tauri.conf.json).
 *
 * Vimeo: requires oEmbed API call (https://vimeo.com/api/oembed.json)
 * to get thumbnail_url — deferred to a follow-up (#1902 v2 / #1904).
 * Returns null for Vimeo in v1; caller should render a provider placeholder.
 *
 * ADR-0050-adjacent; egress tradeoff documented in #1902 PR body.
 */
export function providerThumbnail(
  provider: ResolvedEmbed['provider'],
  videoId: string,
): string | null {
  if (provider === 'youtube') {
    return `https://img.youtube.com/vi/${videoId}/hqdefault.jpg`;
  }
  // Vimeo: oEmbed API call required — deferred
  return null;
}
