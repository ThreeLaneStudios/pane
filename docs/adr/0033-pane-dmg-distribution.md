# ADR-0033: Pane DMG Distribution — paneapp.ai-Hosted, Three Surfaces

**Status:** Accepted  
**Date:** 2026-05-10  
**Author:** Matt Carano, Claude (spec session)  
**Supersedes:** None  
**Supplements:** ADR-0030 (paneapp.ai Stripe Return-URL Contract), ADR-0031 (paneapp.ai Signup Flow), ADR-0032 (paneapp.ai Stack and Hosting)  
**Related:** None

## Context

ADR-0031 step 9 says "Customer downloads the Pane DMG, installs, signs in" — implying a download surface adjacent to the post-signup flow — but does not pin where the DMG lives, how the customer reaches it, or how versioning works. ADR-0030 pins the functional contracts of the three return pages but is explicit that those pages are informational only.

This leaves three open questions:

1. **Where does the DMG live** — paneapp.ai-hosted, GitHub Releases, S3, third-party CDN?
2. **How does the customer reach it** — link on `/subscribe/success`, email, separate page, all of the above?
3. **How are versions handled** — versioned URLs, "always latest" URL, or both?

A drift between ADR-0030 and ADR-0031 surfaced during the paneapp.ai spec session: ADR-0030's `/subscribe/success` contract reads as "informational only" ("Open Pane to continue"), while ADR-0031 step 9 implies the customer downloads from that page. Both readings are defensible; the build session needs a clear call.

### What if we didn't decide

- **The DMG download path gets invented at build time.** Brit/Stia put the DMG on GitHub Releases or some other surface because it's the easiest path during the sprint, and the customer-facing experience drifts from the brand-consistency goal Matt has stated.
- **The `/subscribe/success` ↔ ADR-0031 step 9 drift gets resolved silently.** Without an explicit call, the build session picks one reading (probably whichever they hit first) and the other path quietly stops being supported. Future scope-creep questions ("can we add the download to the receipt email?") have no documented baseline.
- **Versioning policy gets bound late.** Either v1 ships with versioned URLs that carry forward operational complexity (every release updates a path), or with "always latest" that loses the ability to pin (e.g., to a specific build for a customer with a known-bad install). Either is fine; deciding on purpose is better than deciding by accident.

## Decision

**The Pane DMG is hosted on paneapp.ai via Cloudflare Pages, served at `https://paneapp.ai/Pane.dmg` (always-latest), surfaced through three customer touchpoints.**

### Three load-bearing decisions

#### 1. Hosting: paneapp.ai-hosted on Cloudflare Pages

The DMG ships as a static asset alongside the rest of the paneapp.ai deployment. Same vendor, same domain, same deployment pipeline.

- Single-file size limit: 25 MB on Cloudflare Pages free tier
- Pane DMG as of v0.65.18: ~11 MB (well under)
- Bandwidth: unmetered on Cloudflare Pages free tier

If the DMG approaches 25 MB in the future, switch to Cloudflare R2 (S3-compatible object storage) with a custom domain (e.g., `dl.paneapp.ai`). Out of scope for v1; document at the time.

#### 2. URL: always-latest at `/Pane.dmg`

```
https://paneapp.ai/Pane.dmg  →  always serves the current released version
```

A new release replaces the file via paneapp.ai redeploy. Cloudflare Pages handles cache invalidation on deploy automatically.

No versioned URLs in v1 (e.g., `/Pane-v0.65.18.dmg`). Customers always get latest. Versioned URLs become useful when (a) auto-update infrastructure exists and needs to point at specific versions, or (b) a customer-support situation requires pinning a customer to a known-good build. Both are post-launch.

#### 3. Three surfaces, one URL

The DMG is reachable from exactly three places:

| Surface | When the customer sees it | Owner |
|---|---|---|
| `/subscribe/success` page | Immediately after Stripe Checkout completes (first signup or re-subscribe) | paneapp.ai |
| Stripe receipt email | When Stripe sends the customer their receipt | Stripe (configured by wrapper at Checkout Session creation) |
| `/download` public page | Organic visitors, customers re-installing on a new machine | paneapp.ai |

Belt-and-suspenders. Three surfaces; one underlying URL (`https://paneapp.ai/Pane.dmg`).

The `/subscribe/success` placement resolves the ADR-0030 ↔ ADR-0031 drift in favor of the reading that the static download CTA on `/subscribe/success` does not violate ADR-0030's "Must not make API calls / store tokens / verify subscription state" — a download link is just a hyperlink to a static asset, not state mutation.

The Stripe receipt email path uses Stripe's `custom_fields` or `custom_text` on the Checkout Session (or `payment_intent_data` for the receipt) to embed a download link. The wrapper sets this at Checkout Session creation; paneapp.ai never touches Stripe's email surface directly.

The `/download` page is a thin landing page (brief copy, download CTA, system requirements). It is reachable without subscription. This is acceptable because Pane gates access at sign-in (per `docs/spec/onboarding.md` §6.1.1) — the DMG itself is not the gate. A non-subscriber who downloads and runs lands at S1c "no account found → Get Pane," routing them back to paneapp.ai's signup form.

## Rationale

### paneapp.ai-hosted vs. GitHub Releases

GitHub Releases was the obvious alternative — free, well-trodden, the team uses it for builds historically. Rejected for three reasons:

- **Brand consistency.** A customer's download bar shows the source of the file. `paneapp.ai/Pane.dmg` reinforces brand; `github.com/.../releases/download/.../Pane.dmg` introduces a third-party domain mid-flow. For a customer who just paid $29, the polish difference is not zero.
- **Single point of monitoring.** paneapp.ai is already a Cloudflare Pages dependency. Adding GitHub as a second runtime dependency for download means watching two status pages. Cloudflare-only is operationally simpler.
- **Full control of the customer download experience.** If we want to add a `/download` page with system requirements, install instructions, or links to support content, that page is a paneapp.ai page regardless of where the DMG lives. Hosting the DMG on the same domain consolidates ownership.

GitHub Releases remains a perfectly fine option for distributing builds to internal contributors, beta testers, or other audiences. The customer-facing distribution lives on paneapp.ai.

### Three surfaces vs. one

A single surface (e.g., only `/subscribe/success`) was considered. Rejected because:

- Customers who don't download immediately at the success page (closed the tab, distracted, planning to install later) need a way to find the DMG. Receipt email is the durable copy.
- Customers re-installing on a new machine (already paying, just need the DMG) shouldn't have to dig through their email or re-trigger the success page. Public `/download` covers this.
- Belt-and-suspenders is cheap. The cost of three surfaces is one extra page (`/download`) and one Stripe Checkout configuration field. The cost of a customer who can't find the DMG is a support ticket or a churn event.

### Always-latest URL vs. versioned

Versioned URLs (e.g., `/Pane-v0.65.18.dmg` plus `/Pane.dmg → /Pane-v0.65.18.dmg` redirect) were considered. Rejected for v1 because:

- v1 has no auto-update infrastructure. Customers update by manually re-downloading from `/download`. They will always want the latest.
- v1 has no support for pinning a customer to a specific version (no in-app version selector, no versioned download UI on `/download`). Versioned URLs would exist with no surface to drive customers toward them.
- Adding versioned URLs post-launch is a strict superset of the current decision — `/Pane.dmg` continues to work as always-latest, and `/Pane-v0.65.18.dmg` paths get added when needed.

The sprint discipline is "ship the simplest thing that meets the customer need." Versioned URLs become useful when there's a feature that needs them; they aren't useful in their absence.

## Consequences

### Positive

- **Single-vendor download stack.** Cloudflare Pages serves marketing pages, return pages, the DMG, and (eventually) analytics. Operationally simple.
- **Brand consistency.** Customer never leaves paneapp.ai during the post-purchase experience.
- **Free at launch traffic.** Cloudflare Pages free tier covers v1 download volume comfortably.
- **CDN by default.** First-launch download from anywhere in the world hits Cloudflare's edge.
- **Post-launch versioned URLs are additive, not migrative.** Adding `/Pane-vX.Y.Z.dmg` paths later doesn't break the always-latest path.

### Negative

- **DMG ships in the paneapp.ai deployment.** A new release of Pane requires a paneapp.ai deploy to swap the DMG. This is acceptable (deploys are cheap on Cloudflare Pages) but couples the two release cycles. Mitigation: a CI workflow on paneapp-ai repo that pulls the latest DMG from a known location (e.g., a release artifact) and triggers a Pages redeploy. Sub-task to file pre-launch.
- **Cache invalidation timing.** Cloudflare Pages purges cache on deploy, but edge propagation takes seconds-to-minutes. A customer who downloads during a deploy window may get the prior version. Acceptable for v1 (worst case: customer gets version N-1, which is fully functional and self-updates would resolve in future). Document this so it isn't a surprise during release.
- **No version-pinning capability for support cases.** A customer-support situation that requires "please install version X" has no clean path in v1. Mitigation: post-launch sub-task to add versioned URLs alongside the always-latest path.

### Risks

- **DMG size growth.** Pane is ~11 MB now; if it grows above 25 MB, Cloudflare Pages free tier blocks the upload. Mitigation: monitor DMG size in CI; if approaching 20 MB, plan migration to R2 before crossing the limit. Filed as a watch-item; not a launch concern.
- **Customer downloads version-mismatched DMG.** If the wrapper's webhook handlers ever depend on a specific Pane version (e.g., a contract change that requires updated client logic), and a customer downloads pre-update, they could hit a broken state. Mitigation: ADR-0027 / ADR-0029 contract changes are versioned (`/v1/...`); breaking changes ship under `/v2/...` with overlap. v1 launch carries no such version dependency.
- **Cloudflare Pages outage during launch week.** paneapp.ai down means new customers can't download. Mitigation: Cloudflare's 99.99% SLA + status page monitoring. Acceptable bounded risk.

## Alternatives Considered

**GitHub Releases.** Free, well-trodden. Rejected for brand-consistency and single-vendor reasons in §Rationale.

**S3 / Cloudflare R2 with `dl.paneapp.ai` subdomain.** Object storage is the canonical pattern for binary distribution at scale. Rejected for v1 because Cloudflare Pages is sufficient at our size and adding R2 introduces operational surface (bucket policies, CORS, custom domain config) for marginal benefit. R2 becomes the right migration target when DMG size or bandwidth crosses Pages' comfortable range.

**Direct customer email with download link (no `/subscribe/success` CTA).** Considered: rely entirely on the Stripe receipt email for download discovery. Rejected because (a) Stripe receipt timing is not instant — there's a delivery window — and customers expect immediate access after payment, and (b) email deliverability is a real failure mode (spam folders, blocked domains).

**Auto-launch Pane via custom URL scheme (`pane://`) on `/subscribe/success`.** Considered as a UX polish. Rejected for v1 because the customer doesn't have Pane installed yet at this point — the deep-link would 404 in their browser. Becomes useful post-install (e.g., `/account` page deep-linking back to Pane on Portal return), tracked in §8 of the site spec as post-launch.

**Versioned URLs from launch.** Considered. Rejected for v1 per §Rationale (no auto-update, no version-pinning UI, additive post-launch).

**Hosting the DMG inside the wrapper repo (`pane-ecom-backend`) and serving from there.** Considered briefly. Rejected because the wrapper is a backend service, not a static-file host; conflating those responsibilities is exactly the architectural drift ADR-0028 and ADR-0030 are guarding against.

## References

- ADR-0030 — paneapp.ai Stripe Return-URL Contract (the page contracts the download CTA must respect)
- ADR-0031 — paneapp.ai Signup Flow (step 9 implies post-success download)
- ADR-0032 — paneapp.ai Stack and Hosting (Cloudflare Pages baseline this ADR builds on)
- Site spec: `docs/spec/paneapp-ai.md` §6
- Cloudflare Pages limits: https://developers.cloudflare.com/pages/platform/limits/
- Implementation issue: `pane-ecom-backend#13`
- Sub-tasks named by this ADR:
  1. CI workflow on `paneapp-ai` repo to pull latest DMG from build artifacts and redeploy paneapp.ai on new Pane releases
  2. Monitor DMG size in CI; alert if approaching 20 MB (R2 migration trigger)
  3. Configure Stripe Checkout Session to embed `paneapp.ai/Pane.dmg` link in receipt email (wrapper-side)
  4. Document cache-invalidation timing in launch-day runbook
  5. Post-launch: add versioned URLs alongside always-latest path
