# ADR-0050: Link Rich-Embed Security Stance (third-party inline players)

**Status:** Accepted
**Date:** 2026-06-30
**Accepted:** 2026-06-30
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Relates to:** ADR-0049 (Amendment A gates this), #401 (attachment rendering — Link embed mode), #1774 (locked design — Link mode 3), #1780 (og: CSP/capability gate — shares the app CSP this ADR edits), file-system §2.1A (render-mode enum; `link` includes `embed`)
**`spec-covered-by:` Session: ADR-0050 (embed-mode security)**

## Context

The locked #1774 design gives a Link attachment three modes: plain address, preview card (default), and **rich embed** — a live third-party player (YouTube/Vimeo) rendered inline in the note. ADR-0049 Amendment A accepted the HTML-sandbox (D1) and og:-fetch (D2) stances but **explicitly gated embed**: it is a categorically different trust surface from both. D1 renders a *local file* in an *empty* sandbox (no scripts, no network). D2 is a *server-side* metadata fetch. Embed is **live remote content that must run scripts and talk to its provider to function** — the opposite of an empty sandbox, and not a server-side fetch. We have decided to ship embed at launch, so it needs its own codified stance before #401 builds it.

Why a new ADR rather than folding into #1780: #1780 is the CSP/capability gate for **outbound HTTP** (the og: fetch / `connect-src` / `img-src` surface). Embed is a **client-side live iframe** surface — `frame-src`, third-party script execution inside the WebView, provider allowlisting, sandbox flags. Different threat model, different enforcement point. They share only the app CSP file (this ADR adds `frame-src`; #1780 handles `connect-src`/`img-src`), cross-referenced below.

## Decision

Link embed mode renders an allowlisted third-party video player in a constrained cross-origin iframe, built from a canonically reconstructed provider URL, never auto-applied. Six requirements, all mandatory:

### 1. Opt-in per attachment

Embed is never the default and never auto-applied. Preview card stays the Link default (file-system §2.1A). The user explicitly selects embed on a specific Link attachment. A Link in any other mode performs no embed and loads no provider iframe.

### 2. Provider allowlist + canonical reconstruction (default-deny)

Only known video providers may render as an embed. v1 allowlist:

- **YouTube** (`youtube.com`, `www.youtube.com`, `m.youtube.com`, `youtu.be`)
- **Vimeo** (`vimeo.com`, `player.vimeo.com`)

A single resolver (`resolveEmbed(url)`) parses the user's pasted URL, matches it against the allowlist, extracts the **video id**, and **reconstructs the canonical embed URL** from a trusted template — it never passes the raw user URL through as the iframe `src`:

- YouTube → `https://www.youtube-nocookie.com/embed/{id}` (privacy-enhanced domain — verified)
- Vimeo → `https://player.vimeo.com/video/{id}?dnt=1` (Do-Not-Track — verified)

If the URL matches no allowlisted provider, or no id can be extracted, `resolveEmbed` returns `null` and the attachment **falls back to preview** (or plain). No arbitrary-origin URL is ever embedded. Reconstruction (not passthrough) prevents crafted-URL attacks (look-alike hosts, injected query/path).

### 3. Iframe sandbox = minimum to play

```
sandbox="allow-scripts allow-same-origin"
allow="encrypted-media; picture-in-picture; fullscreen"
referrerpolicy="strict-origin-when-cross-origin"
```

- `allow-scripts allow-same-origin` is **required** — without `allow-same-origin` the player fails (`SecurityError: ... lacks the 'allow-same-origin' flag`; verified). These are the *only* sandbox tokens granted.
- **Deliberately omitted:** `allow-popups`, `allow-popups-to-escape-sandbox`, `allow-top-navigation`(`-by-user-activation`), `allow-modals`, `allow-forms`, `allow-downloads`. The embed cannot open windows, navigate Pane away, or prompt.
- Player capabilities (fullscreen, DRM, PiP) ride the separate `allow=` Permissions-Policy attribute, **not** the sandbox. **Autoplay is omitted** — nothing plays until the user clicks. (The brief's `allow-presentation` sandbox token is the Presentation/casting API and is *not* required for playback or fullscreen — omitted.)

### 4. Why allow-scripts + allow-same-origin is acceptable here (and never for D1)

The standard warning — "allow-scripts + allow-same-origin lets the frame remove its own sandbox" — is a **same-origin** exploit: the frame's script reaches `window.frameElement` to strip the attribute, or `window.parent` to touch the host DOM. **Both require same-origin with the host.** Here the framed origin is a **remote third party** (youtube-nocookie.com / player.vimeo.com), cross-origin to Pane's WebView origin, so:

- `window.frameElement` is inaccessible (cross-origin) → the embed cannot remove its own sandbox.
- `window.parent` DOM access is SOP-blocked → the embed cannot read or alter the note.
- The embed runs scripts only within *its own* provider origin (which is what playback needs), with that origin's own cookie/storage jar — not Pane's.

This is the inverse of D1, where the content is a **local file** that, with `allow-same-origin`, would share Pane's own origin context — which is exactly why D1 forbids it and uses an empty sandbox. **The two stances must never be conflated:** local untrusted file → empty sandbox; allowlisted remote provider → `allow-scripts allow-same-origin`, contained by the cross-origin boundary.

### 5. Tauri IPC containment

The provider iframe must have **no** access to Tauri IPC or capabilities. Confirm `tauri.conf.json` does **not** grant remote-domain IPC (no remote origins in `app.security.capabilities`; v1's `dangerousRemoteDomainIpcAccess` equivalent absent). A cross-origin iframe does not receive Tauri's injected APIs by default; this requirement makes that explicit and auditable, so an allowlisted-but-compromised provider still cannot reach Pane's backend. The blast radius stays inside the provider's own origin.

### 6. CSP frame-src allowlist (default-deny) + disclosed, toggleable egress

- App CSP `frame-src` lists **only** `https://www.youtube-nocookie.com https://player.vimeo.com`; everything else is denied by default. (This edits the same `tauri.conf.json` CSP that #1780 touches for `connect-src`/`img-src` — coordinate, don't duplicate.)
- Loading an embed **beacons the provider** (a live connection, stronger egress than the preview fetch). This is inherent and user-initiated. It is **disclosed** and **toggleable**: a setting `link_embeds_enabled` (`ai_settings`, default `'true'`, disclosed in Settings, separate from `link_previews_enabled`). When off, embed mode is unavailable and Links render as preview/plain with no provider connection.

**Enforcement point:** `resolveEmbed()` (provider allowlist + canonical URL reconstruction) and `AttachmentLinkEmbed.svelte` (the sole component that mounts a provider iframe, with the fixed sandbox/allow/referrerpolicy above) are the single enforcement point. `AttachmentNodeView` routes `renderMode === 'embed'` **only** to `AttachmentLinkEmbed`, and only after `resolveEmbed` returns non-null. Any other path that mounts a remote iframe is a violation. (Mirrors ADR-0049's single-enforcement-point pattern.)

## Consequences

- Embed works for the two providers users actually paste (YouTube/Vimeo) while every other URL safely degrades to preview.
- A compromised or malicious allowlisted provider is contained to its own origin: no Pane DOM access, no sandbox self-removal, no Tauri IPC, no popups/navigation.
- Privacy cost (provider beacon on load) is disclosed, toggleable, and minimized (nocookie/dnt endpoints, no autoplay, no referrer leakage).
- New setting key `link_embeds_enabled`; a `frame-src` addition to the app CSP (coordinated with #1780).
- Cost: providers outside the allowlist can't be embedded (acceptable — preview + open-externally cover them; the allowlist is extended deliberately, not by user input).

## Alternatives considered

- **Allow arbitrary-origin embeds (any URL the user upgrades).** Rejected — unbounded third-party-script + frame-src surface, trivially abused by a crafted link; defeats the point of an allowlist.
- **Empty sandbox (reuse D1's stance).** Rejected — an empty sandbox cannot run the player; embed would never function. The modes are not interchangeable.
- **Server-side proxy / re-host the video.** Rejected for v1 — heavy (bandwidth, transcode, storage), and unnecessary given cross-origin containment + provider allowlist.
- **Pass the user URL straight to the iframe.** Rejected — look-alike hosts and injected path/query become an attack surface; canonical reconstruction from an extracted id is required.
- **Fold into #1780.** Rejected — different surface (frame-src + client iframe vs outbound HTTP); a combined decision muddies both. Cross-referenced instead.

## Risks

- **Allowlist/parsing bypass** (look-alike host like `youtube-nocookie.com.evil.tld`, or id-extraction tricked into a foreign origin). Mitigation: match on exact registrable-domain set, extract id with strict patterns, reconstruct from a fixed template; unit tests for look-alike hosts and malformed ids → must return `null` (fall back to preview).
- **Tauri grants IPC to remote frames via future config drift.** Mitigation: requirement 5 + an INVARIANTS entry; a CI/config check that no remote origin appears in capabilities.
- **CSP `frame-src` widened carelessly later.** Mitigation: INVARIANTS entry pins the default-deny allowlist; additions are a reviewed ADR amendment.
- **Provider changes its embed contract** (endpoint, required tokens). Mitigation: `resolveEmbed` templates are the single point of change; verified against the provider surface as of 2026-06-30 (see session notes) with a re-verify note.

## References

- ADR-0049 Amendment A (the gate this ADR satisfies); D1 (the empty-sandbox stance this is explicitly distinct from).
- #1774 locked design — Link mode 3 (rich embed, "play video links inline").
- file-system §2.1A — render-mode enum (`link`: `preview` (default) | `plain` | `embed`). Picker renders in schema order: preview → plain → embed.
- #1780 — og: CSP/capability gate (shared `tauri.conf.json` CSP; `connect-src`/`img-src`).
- Verified provider surface (2026-06-30): YouTube `youtube-nocookie.com/embed/{id}` + required `allow-same-origin`; Vimeo `player.vimeo.com/video/{id}?dnt=1`. Sources in session notes.
- Tracking: embed-mode line item on #1521 launch tracker.
