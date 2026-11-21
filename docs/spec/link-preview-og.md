# Link Preview — og: Enrichment

**Status:** BUILT v0.67.90  
**Relates to:** #1780, ADR-0049 Decision 2, #1902, #1898, #401  
**Date:** 2026-07-04

---

## Summary

When a user pastes a URL as a link attachment, Pane fetches Open Graph metadata (`og:title`, `og:description`, `og:image`) server-side (Rust) and displays it in the link preview card. Non-video links (where `resolveEmbed(url) === null`) receive og: enrichment; YouTube/Vimeo links continue to render the video card (#1902 path).

---

## Architecture

### Fetch path

1. **On insert** (`Editor.svelte → handleLinkModalSubmit`): stub node inserts immediately with domain stub; `fetchLinkPreview(url)` fires and-forget.
2. **On card render** (`AttachmentNodeView.svelte → $effect`): calls `getCachedLinkPreview(url)` first; on cache miss triggers `fetchLinkPreview(url)` in background.
3. **Rust command** (`og_fetch.rs → fetch_link_preview`): validates scheme, resolves DNS, applies SSRF blocklist, issues HTTP request, parses og: tags, writes to `og_cache`.
4. **Svelte state**: `ogTitle`, `ogDescription`, `ogImage` reactive state in `AttachmentNodeView` updates `AttachmentCard` props reactively.

### Single enforcement point

`src-tauri/src/og_fetch.rs::fetch_link_preview` is the **sole** fetch path. The frontend never fetches arbitrary URLs for unfurl (per ADR-0049 Decision 2).

---

## SSRF Guard (ADR-0049 Decision 2 — non-negotiable)

All guards applied per request hop (initial URL + every redirect `Location`):

| Guard | Implementation |
|---|---|
| Scheme allowlist | `parse_and_validate_url()` — only `http`/`https`; rejects `file:`, `data:`, `ftp:`, `javascript:`, all others |
| DNS pre-resolution | `tokio::net::lookup_host` called before each HTTP request |
| Loopback block | `127.0.0.0/8`, `::1` |
| RFC1918 private | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` |
| Link-local | `169.254.0.0/16` (includes `169.254.169.254` cloud metadata), `fe80::/10` |
| Unique-local IPv6 | `fc00::/7` |
| IPv4-mapped IPv6 | `::ffff:x.x.x.x` — normalized to v4 before check |
| Multicast | `224.0.0.0/4` (v4), `ff00::/8` (v6) |
| Broadcast | `255.255.255.255` |
| Redirect re-validation | Manual redirect follow (`reqwest::redirect::Policy::none()`); each hop re-resolved + re-checked |
| Redirect cap | ≤3 hops |
| Timeout | ≤5s wall-clock + connect timeout (both) |
| Body cap | ≤256 KB streaming (stops reading past cap; no OOM) |
| Content-Type gate | Only `text/html`; returns `no_og` for all others |
| No credential leakage | Fresh `reqwest::Client` per fetch; no cookies, no auth headers, no app tokens |

---

## Cache (og_cache table)

**Schema** (migration v69, `migrations.rs`):

```sql
CREATE TABLE og_cache (
    url             TEXT PRIMARY KEY NOT NULL,
    og_title        TEXT,
    og_description  TEXT,
    og_image        TEXT,
    fetched_at      INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT 'ok'
    -- status: 'ok' | 'timeout' | 'blocked' | 'no_og' | 'error'
);
```

**TTL policy** (`og_fetch.rs`):
- `status = 'ok'` → 7 days
- All other statuses (timeout, blocked, no_og, error) → 1 hour

Failed fetches are cached (short TTL) to prevent re-hammering on every render.

---

## Setting gate

**Key:** `link_previews_enabled` (ai_settings, default `'true'`)

Replaces the orphaned `link_embeds_enabled` key (which gated the now-removed embed path). Migration v69 carries over existing `link_embeds_enabled` values for users who had toggled it off.

When `link_previews_enabled = 'false'`, `fetch_link_preview` returns `null` and no fetch is made.

**Settings UI:** "Enable link previews (og: metadata enrichment)" with disclosure that a server-side request is made to the linked site.

---

## CSP and image egress

**Security amendment (#2181):** production `img-src` is allowlisted rather than permitting generic `https:`:

```
img-src 'self' asset: https://asset.localhost blob: data: https://img.youtube.com
```

`https://img.youtube.com` remains the sole remote image origin for canonically reconstructed, allowlisted video cards. Arbitrary `og:image` values remain cached metadata but are not loaded by `AttachmentCard`; link previews use their local icon. A future thumbnail implementation must download, validate, size-cap, and cache the image through a guarded Rust path before exposing a local asset URL.

---

## Failure modes / fallback

All failure paths fall back to the domain stub (`extractDomain(src)` in `AttachmentCard`):

| Failure | Behavior |
|---|---|
| Timeout | `status=timeout`, cache 1h, card shows domain stub |
| SSRF blocklist hit | `status=blocked`, cache 1h, card shows domain stub |
| Non-HTML content | `status=no_og`, cache 1h, card shows domain stub |
| No og tags | `status=no_og`, cache 1h, card shows domain stub |
| Redirect cap exceeded | error, cache 1h, card shows domain stub |
| `link_previews_enabled=false` | No fetch; card shows domain stub immediately |

---

## Boundary with #1902 (video cards)

`resolveEmbed(url) !== null` → YouTube/Vimeo video card (untouched by #1780).  
`resolveEmbed(url) === null` → og-enriched `AttachmentCard` (#1780 path).

Files NOT touched by #1780:
- `src/lib/resolvers/resolveEmbed.ts`
- `src/lib/components/AttachmentVideoCard.svelte`
- `AttachmentNodeView.svelte` video card branch (`{#if ... resolveEmbed !== null}`)

---

## Files changed

| File | Change |
|---|---|
| `src-tauri/src/og_fetch.rs` | New module: SSRF guard, fetch, og parser, cache r/w, Tauri commands |
| `src-tauri/src/lib.rs` | `mod og_fetch;` + register 2 commands |
| `src-tauri/src/migrations.rs` | Schema v69: `og_cache` table + `link_previews_enabled` migration |
| `src-tauri/src/database.rs` | Seed `link_previews_enabled` (replaces `link_embeds_enabled`) |
| `src-tauri/tauri.conf.json` | `img-src` + `https:` |
| `src/lib/components/AttachmentCard.svelte` | Accept `ogTitle`, `ogDescription`, `ogImage` props; render enriched card |
| `src/lib/components/AttachmentNodeView.svelte` | og state + `$effect` + pass og props to AttachmentCard |
| `src/lib/components/Editor.svelte` | Fire-and-forget `fetchLinkPreview` on link insert |
| `src/lib/api.ts` | `fetchLinkPreview`, `getCachedLinkPreview` IPC wrappers |
| `src/lib/components/Settings.svelte` | `link_embeds_enabled` → `link_previews_enabled`; updated UI label |
