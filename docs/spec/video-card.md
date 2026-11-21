# Spec: Video Card (#1902)

**Status:** BUILT — v0.67.90 (feat/1902-youtube-video-card)  
**Relates to:** #1902, #1903, ADR-0050

---

## Overview

Post-#1903 (iframe embed removed — WKWebView opaque origin blocks Referer, Error 153), YouTube and Vimeo link attachments in `link + preview` render mode now show a rich **video card** instead of the thin "File / domain" domain stub.

The video card is an additive affordance within the existing `link + preview` render path. No new render mode is introduced; `VALID_RENDER_MODES.link` remains `['preview', 'plain']`.

---

## Visual Design

- **Height:** 64px — same as `AttachmentCard` canonical card height (`--at-card-h`)
- **Layout (left → right):**
  1. **Thumbnail tile** (38×38 px) — YouTube CDN image with play-icon overlay; Vimeo placeholder icon (v1)
  2. **Text column** — title (from `attrs.filename` or URL fallback) + provider label ("YouTube" / "Vimeo")
  3. **"Open in [Provider]" button** — launches canonical watch URL in system browser via `tauri-plugin-shell` `open()`

---

## Provider Coverage

| Provider | Thumbnail | Button |
|----------|-----------|--------|
| YouTube  | `https://img.youtube.com/vi/{id}/hqdefault.jpg` (zero-network CDN URL) | `https://www.youtube.com/watch?v={id}` |
| Vimeo    | Placeholder icon — oEmbed API call deferred (#1902 v2) | `https://vimeo.com/{id}` |

Allowlist source of truth: `src/lib/resolvers/resolveEmbed.ts` (`YOUTUBE_HOSTS`, `VIMEO_HOSTS` sets).

Non-allowlisted URLs fall through to the existing `AttachmentCard` domain stub (unchanged).

---

## Routing Logic

```
link + preview + resolveEmbed(src) !== null  →  AttachmentVideoCard
link + preview + resolveEmbed(src) === null   →  AttachmentCard (domain stub, unchanged)
link + plain                                  →  <a class="at-link-plain"> (unchanged)
```

---

## Security

- YouTube thumbnail CDN (`img.youtube.com`) added to `img-src` in `tauri.conf.json`.
- `frame-src` unchanged (dead code left from #1896 — cleanup in separate PR).
- `shell:default` capability already allows arbitrary `https://` open — no capability change.
- ADR-0050 §2 allowlist unchanged; `resolveEmbed.ts` unmodified except the new `providerThumbnail()` helper.

---

## Known Limitations (v1)

- Vimeo thumbnail is not shown (oEmbed API call required — deferred)
- No offline/error fallback for thumbnail load failure (browser default empty-image)
- No og:title fetch (deferred to #1780)
