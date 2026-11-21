# ADR-0049: Attachment HTML Sandbox & Link-Preview Network Egress

**Status:** Accepted
**Date:** 2026-06-29
**Accepted:** 2026-06-30
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Relates to:** #401 (attachment rendering — PR6 HTML preview, PR7 Link type), #1774 (locked design), #1781 (link-preview egress tracking), file-system §2.1/§2.1A
**`spec-covered-by:` Session: Attachments**

## Context

The #401 attachment-rendering build introduces two capabilities that cross a trust boundary and therefore need a codified security stance, not an implementer's judgment call:

1. **HTML attachments** get a preview mode. An HTML file is arbitrary, attacker-controllable markup that can carry scripts, trackers, and network beacons. Pane is local-first; rendering such a file must not become a script-execution or data-exfiltration surface inside the app.
2. **Link attachments** get preview cards built from Open Graph metadata, which requires Pane to **fetch a user-pasted URL from the host machine**. An unrestricted fetch is an SSRF and privacy hole: it can reach loopback/private/link-local addresses and cloud-metadata endpoints, and it leaks that the user saved the link.

Reed's #401 scope notes the sandbox direction (`<iframe sandbox="">`, "never `allow-scripts`") and that the link fetch "requires network from the Rust side — confirm CSP/capability." This ADR makes both stances normative and names the single enforcement point for each, so the invariants can be mirrored into `INVARIANTS.md` and audited.

## Decision

### Decision 1 — HTML attachments render only in an empty-sandbox iframe; never inline-executed

- HTML attachments render **only** inside a sandboxed preview modal (`AttachmentHTMLPreview.svelte`, #401 PR6), in an `<iframe sandbox="">` with an **empty** sandbox attribute. Empty sandbox = no scripts, no forms, no same-origin, no popups, no top-level navigation.
- **`allow-scripts` is never permitted** on an attachment iframe. `allow-same-origin` is excluded by default; it may be added **only** if a concrete relative-CSS rendering need is demonstrated and security-reviewed, and never together with `allow-scripts` (the combination defeats the sandbox).
- HTML attachment content is **never** injected into the note's `contenteditable` DOM and never rendered by any path other than the modal iframe. The collapsed card (default mode) shows metadata only; it does not execute or embed the HTML.
- Content is delivered to the iframe as a `data:`/`srcdoc` payload (opaque origin), which — combined with the empty sandbox — blocks script execution and outbound requests originating from the document.
- The app Content-Security-Policy must not be loosened to accommodate attachment HTML.

**Enforcement point:** `AttachmentHTMLPreview.svelte` is the sole render path; `AttachmentNodeView.svelte` routes `attachmentType === 'html'` only to that component. Any second HTML render path is a violation.

### Decision 2 — Link-preview fetch is server-side, guarded, and bounded

`fetch_link_preview` (Rust, #401 PR7) fetches a user-pasted URL to extract `og:title`, `og:description`, `og:image`, and favicon. It must enforce all of:

- **Scheme allow-list:** `http`/`https` only. Reject `file:`, `data:`, `ftp:`, and all others.
- **SSRF guard:** reject the request if the **resolved** IP is loopback (`127.0.0.0/8`, `::1`), private (`10/8`, `172.16/12`, `192.168/16`), link-local (`169.254/16`, `fe80::/10`), unique-local (`fc00::/7`), or the cloud-metadata address (`169.254.169.254`). Resolve DNS and validate the resolved IP (not just the hostname) to defeat DNS-rebinding. **Re-validate on every redirect hop**; cap redirects (e.g. ≤3).
- **No credential leakage:** do not forward cookies, auth headers, or app tokens. Fresh client, no ambient credentials.
- **Timeout:** hard wall-clock timeout (≤5s). On timeout/error/guard-rejection the card shows a designed **"couldn't load"** state and falls back to the plain address.
- **Byte cap:** read only the HTML `<head>` region up to a small cap (e.g. ≤256 KB); never stream an unbounded body.
- **og:image:** retained as metadata only. Per the #2181 security amendment, arbitrary remote image URLs are **not loaded by the WebView**. Preview cards use a local icon until a guarded, size-capped backend proxy/cache produces a local asset URL.
- **Opt-out:** gated by setting `link_previews_enabled` (`ai_settings`, default `'true'`, disclosed in Settings). When off, links render as plain addresses with no fetch.
- **Capability:** the WebView has no arbitrary outbound HTTP capability. Metadata fetching remains behind the typed Rust command; production `connect-src` is limited to Tauri IPC.

**Enforcement point:** `attachments.rs::fetch_link_preview` — the scheme check, resolved-IP guard, redirect re-validation, timeout, and byte cap all live there; the frontend never fetches arbitrary URLs directly for unfurl.

## Consequences

- HTML previews are inert: safe to open any `.html` a user attaches without script/network risk. Cost: a page relying on JS won't be interactive in-preview (acceptable; "Open externally" remains for full fidelity).
- Link previews can't be weaponized to probe the user's internal network or cloud metadata, and the privacy cost is disclosed and toggleable.
- Two clear, auditable single-enforcement-points; mirrored as invariants (see `INVARIANTS.md` entries).
- A new setting key (`link_previews_enabled`) and a possible `tauri.conf.json` capability addition.

## Alternatives considered

- **`allow-scripts` for "useful" HTML previews.** Rejected — reintroduces arbitrary script execution; the entire point of the sandbox is to forbid it. "Open externally" covers full-fidelity needs.
- **Client-side (WebView) link fetch instead of Rust.** Rejected — harder to apply a resolved-IP SSRF guard and redirect re-validation in the WebView; CSP alone doesn't stop SSRF to private ranges. Server-side fetch with an explicit guard is enforceable and testable.
- **Server-download og:image in v1.** Deferred — adds a second guarded fetch + cache + size policy. Direct URL-by-CSP display was later rejected by the #2181 security amendment because it creates an arbitrary passive egress channel.
- **No link previews at all.** Rejected — the locked #1774 design includes the preview-card mode; this ADR makes it safe rather than dropping it.

## Risks

- **Guard bypass via exotic DNS / IPv6 mapping (e.g. `::ffff:127.0.0.1`, redirect to private after first hop).** Mitigation: validate the resolved IP including IPv4-mapped IPv6, on every hop; unit tests for each blocked range and a redirect-to-private case.
- **`allow-same-origin` creep.** Mitigation: INVARIANTS entry forbids it by default; any addition requires review and is incompatible with `allow-scripts`.
- **Setting drift** (a future code path fetching links while `link_previews_enabled` is off). Mitigation: the gate lives in `fetch_link_preview`, the single fetch entry point.

## References

- Reed #401 rendering scope (2026-06-29): PR6 (HTML sandbox modal), PR7 (Link type / `fetch_link_preview`).
- file-system §2.1 / §2.1A — attachment reference format and render-mode persistence.
- #1774 / `LOCKED-DESIGN-SPEC.md` — render modes (locked).
- Tracking: #1781 (link-preview egress).

---

## Amended — 2026-06-30 (re-read against final #1774 design, Session: ADR-0049 acceptance)

Decisions 1 and 2 are confirmed against the locked #1774 design and accepted as written. Two deltas surfaced on re-read; both are captured here without altering the decisions above.

### A. Link "rich embed" mode is outside this ADR's coverage and needs its own stance before it ships.

The #1774 design gives Link **three** modes: plain address, preview card (default), and **rich embed** ("play video links inline" — a live third-party player, e.g. a YouTube/Vimeo iframe). Decisions 1–2 cover only the HTML-attachment sandbox and the server-side og: *metadata* fetch for the preview card. The rich-embed mode is a different trust surface — **live remote content that requires scripts and network to function** — and Decision 1's empty-sandbox stance explicitly does **not** apply to it (an empty sandbox cannot play an embed).

Until a dedicated decision lands, the rich-embed mode MUST NOT ship. That decision must specify, at minimum:
- **Opt-in only** (per attachment; never auto-embedded — the design already makes preview, not embed, the default).
- **Provider allowlist** — only known video providers get a live iframe; arbitrary URLs cannot be upgraded to embed (fall back to preview/plain). No arbitrary-origin embedding.
- **Privacy-enhanced endpoints** where available (e.g. `youtube-nocookie.com`).
- **Iframe sandbox** = the minimum to play (`allow-scripts allow-same-origin allow-presentation`; not `allow-popups`/`allow-top-navigation`). This `allow-scripts allow-same-origin` pairing is acceptable **only because the framed origin is a remote third party** (its "same-origin" is the provider, not Pane's local origin and not file access) — this is categorically different from Decision 1, where the content is a local file that must stay in an empty sandbox. The two must never be conflated.
- **CSP `frame-src`** allowlist of provider origins; default-deny.
- **Disclosed egress** — embed load beacons to the provider; this is a stronger egress than the preview fetch and must be disclosed/toggleable.

PR6 (HTML preview) and PR7 (link preview/fetch) are unblocked by this acceptance. The rich-embed mode is gated on the follow-up decision (tracked separately — see session notes).

### B. Decision 1's "no network" guarantee requires a document CSP, not the empty sandbox alone.

Decision 1 states the empty sandbox + opaque origin "blocks ... outbound requests originating from the document." That holds for **scripted** requests (no `allow-scripts` ⇒ no `fetch`/`XHR`/script beacons), but **not for passive subresource loads**: a static `<img src="https://tracker/?leak">`, remote `<link rel=stylesheet>`, `<iframe>`, or web-font in attacker-controlled `.html` still issues a network request with no script. To deliver the design's stated "scripts and network are disabled," the rendered document MUST also carry a restrictive Content-Security-Policy (`default-src 'none'`; permit only inert needs, e.g. `style-src 'unsafe-inline'`, `img-src data:`), applied by Pane to the sandboxed document (CSP `<meta>` in the `srcdoc`, or an equivalent header) — sanitization of remote references is an acceptable alternative/addition. Enforcement remains `AttachmentHTMLPreview.svelte`. INVARIANTS `INV-ATT-HTML-SANDBOX` is strengthened accordingly (see INVARIANTS edit).

This refines the enforcement mechanism; it does not change Decision 1's stance (HTML stays inert and non-exfiltrating).
