# ADR-0032: paneapp.ai Stack and Hosting — SvelteKit + Cloudflare Pages

**Status:** Accepted  
**Date:** 2026-05-10  
**Author:** Matt Carano, Claude (spec session)  
**Supersedes:** None (supersedes the archived `docs/archive/website-spec.md` direction informally)  
**Supplements:** ADR-0028 (Ecom Platform — Supabase + Stripe + thin wrapper)  
**Related:** ADR-0030 (paneapp.ai Stripe Return-URL Contract), ADR-0031 (paneapp.ai Signup Flow)

## Context

paneapp.ai is launch-blocking (`pane-ecom-backend#13`) and must serve three load-bearing surfaces: marketing landing page, signup form, three Stripe return pages. The signup form requires server-side capability (per `docs/spec/paneapp-ai.md` §2 service role boundary — paneapp.ai must not hold the Supabase service role key, but it must call a service-role-bearing endpoint to create users). The return pages are brochure-only (ADR-0030).

Stack and hosting selection was deferred from the platform-decision ADR (ADR-0028), which scoped to the wrapper. This ADR closes the remaining seam: what runs the customer-facing site.

The constraints:

- Must integrate Stripe Checkout via Stripe.js
- Must integrate Supabase Auth via `@supabase/supabase-js` (for the sign-in recovery surface; signup itself goes through the wrapper)
- Must support env-var management for Stripe publishable key, Supabase URL/anon-key, and an optional shared secret for the wrapper signup endpoint
- Must support fast deploys (PR previews, push-to-deploy on main) — the team operates in this idiom on Pane already
- Must be implementable by Brit + Stia within the launch window — favor stacks they already know
- Static-first preferred where possible (return pages especially have no state)

### What if we didn't decide

Stack-selection drift would push the question into the build session. Three failure modes:

- **Brit/Stia pick a framework they don't know well.** Cost shows up as integration friction and missed launch deadline. A spec-time pick lets them start in a familiar idiom.
- **The hosting choice gets bound late.** Hosting choice constrains framework choice (e.g., Cloudflare Pages plays best with adapters that target Cloudflare Workers; some Next.js features don't deploy cleanly to non-Vercel hosts). Picking late means rework.
- **Conformance with `pane-ecom-backend` repo discipline drifts.** The wrapper landed with explicit ADR-0023 captures and Reed coverage. paneapp.ai needs the same discipline; spec-time decisions establish that floor.

## Decision

paneapp.ai is built on **SvelteKit** (latest stable, 2.x) and deployed to **Cloudflare Pages**.

### Three load-bearing decisions

#### 1. Framework: SvelteKit

SvelteKit is Pane's frontend framework; Brit and Stia operate in it daily. paneapp.ai's content shape (mostly static, one server endpoint for signup) maps cleanly to SvelteKit's hybrid rendering model. TypeScript is first-class. The Cloudflare Pages adapter (`@sveltejs/adapter-cloudflare`) is officially supported.

#### 2. Hosting: Cloudflare Pages

Free tier covers v1 launch traffic with unmetered bandwidth. Edge CDN by default. GitHub-native deploys (push to main → auto-deploy; PRs get preview deployments). Cloudflare Workers / Pages Functions support gives paneapp.ai server-side capability for the signup endpoint without a separate hosting provider.

#### 3. Service role boundary

paneapp.ai never holds the Supabase service role key. Signup flows through a wrapper endpoint (`POST /v1/auth/signup`, anonymous like `/v1/auth/email-check`) that performs both writes (Supabase Auth user + `ecom.user_lookup` row) atomically server-side. This is a new wrapper endpoint outside ADR-0027's five-endpoint shape; coordination item filed against `pane-ecom-backend#2`.

### Out-of-band specifics in `docs/spec/paneapp-ai.md`

The site spec covers the implementation details that don't rise to ADR level: env vars, libraries, CI shape, deploy mechanics. This ADR pins the framework + hosting + service-role-boundary decisions; the spec carries the implementation surface.

## Rationale

### SvelteKit specifically

- **Team familiarity.** Brit and Stia have shipped Pane's frontend in Svelte. No framework-onboarding cost.
- **Hybrid rendering fits the shape.** Marketing pages and return pages render statically; the signup form uses a SvelteKit server endpoint to call the wrapper. SvelteKit handles both in one codebase without per-page configuration ceremony.
- **First-class Cloudflare adapter.** `@sveltejs/adapter-cloudflare` is officially maintained and well-documented.
- **TypeScript-first.** Matches Pane's frontend; type-checking carries across the stack.

### Cloudflare Pages specifically

- **Free at launch traffic.** Cloudflare Pages free tier: unlimited sites, 500 builds/month, unmetered bandwidth. Launch-week 100 customers don't approach any limit.
- **Edge CDN.** Customer's first DMG download benefits from Cloudflare's global edge network. Static assets (the DMG itself, marketing page assets) cache automatically.
- **GitHub-native CI.** Push to `main` → auto-deploy. PRs → branch preview URLs. No additional CI infrastructure to set up.
- **Pages Functions.** Server-side endpoints (e.g., signup-form submission proxying to the wrapper) run as Cloudflare Workers, deploy with the rest of the site, share the same env-var store. No second hosting provider.
- **Same vendor as analytics.** Cloudflare Web Analytics is cookieless and free; if added post-launch, integration is one toggle in the Cloudflare dashboard.
- **Continuous with archived prior decision.** `docs/archive/website-spec.md` had already converged on Cloudflare Pages for paneapp.ai before being archived. Bringing that decision forward, formalized via this ADR.

### Service role boundary specifically

The Supabase service role key is admin-equivalent: it can write any user's `app_metadata`, including subscription status. A leak from a customer-facing site is a full ecom takeover.

paneapp.ai must call a service-role-bearing endpoint, but must not hold the key itself. The wrapper is the natural chokepoint — it already holds the key (per ADR-0028) and already exposes anonymous endpoints (`/v1/auth/email-check`). Adding `POST /v1/auth/signup` is the symmetric extension.

The alternative — paneapp.ai using Supabase's public `auth.signUp()` and a `SECURITY DEFINER` RPC function for the `user_lookup` write — was considered. Rejected because it scatters the ecom-write boundary across two surfaces (Supabase RPC + wrapper) instead of consolidating it on the wrapper, and because it complicates the rate-limiting story (Supabase Auth rate limits sign-ups per IP, but `user_lookup` writes via RPC are a separate primitive that needs its own throttling).

## Consequences

### Positive

- **Brit and Stia start in familiar territory.** SvelteKit + TypeScript matches Pane's frontend. No framework-onboarding cost.
- **One vendor for site + edge functions + CDN + analytics.** Cloudflare Pages handles all four. Operational simplicity.
- **Free at launch traffic.** No fixed hosting cost on top of Stripe transaction fees.
- **Service role stays inside the wrapper.** paneapp.ai is pure frontend. Compromise of Cloudflare Pages env vars (publishable Stripe key + anon Supabase key) does not enable ecom takeover.
- **PR preview deploys for design review.** Brit + Stia ship a branch; Matt opens the preview URL; design feedback happens on a real deployed site, not a screenshot.

### Negative

- **One more repo for Reed to cover.** `paneapp-ai` repo joins `pane` and `pane-ecom-backend` under Reed's PR-review scope. Sub-task: extend Reed coverage as a precondition for paneapp.ai PRs landing.
- **New wrapper endpoint outside ADR-0027's five-endpoint shape.** `POST /v1/auth/signup` is a contract addition. Either ADR-0027 gets a partial-supersession (like ADR-0029 did for §2 and §3) or a new ADR pins the new endpoint. Recommend a follow-up ADR for the signup endpoint contract before the wrapper builds against it. Not blocking this ADR.
- **Cloudflare Pages free-tier ceiling.** 500 builds/month is comfortable for launch but bears watching if PR-preview activity spikes or auto-deploys on main run hot. Mitigation: monitor build count post-launch; upgrade plan ($20/month for 5,000 builds) is trivial.

### Risks

- **Cloudflare Pages outage.** Cloudflare's edge has had public incidents (June 2022, July 2024). paneapp.ai outage during a Cloudflare incident means customers cannot sign up. Mitigation: monitor Cloudflare status; the wrapper and existing customer Pane installs are unaffected (different infrastructure). Acceptable bounded risk for v1.
- **Supabase Auth API drift.** paneapp.ai's sign-in recovery surface uses `@supabase/supabase-js` directly. ADR-0023 capture discipline applies: re-verify on observed breaks. The existing capture (`docs/spec-prep/2026-05-07-supabase-auth-platform-capture.md`) covers the Auth surface; pre-launch re-verification is sub-tasked under ADR-0028.
- **Stripe.js version pinning.** Stripe.js is loaded from `js.stripe.com/v3` — Stripe's recommended pattern. The version is not pinnable client-side; Stripe ships breaking changes via API-version pinning, not script-version. The wrapper-side capture (`docs/spec-prep/2026-05-07-stripe-platform-capture.md`) handles this; paneapp.ai inherits.

## Alternatives Considered

**Astro + Svelte islands.** Best-in-class for content-heavy sites with interactive islands. Rejected because paneapp.ai's content is small enough that the islands architecture is unneeded ceremony. SvelteKit's static rendering is sufficient for the marketing pages, and the signup form needs a full SvelteKit server endpoint anyway, so the islands benefit doesn't compound.

**Next.js on Vercel.** Largest ecosystem; Vercel is the canonical Next.js host. Rejected because Brit and Stia don't operate in React idiomatically, and Vercel introduces another vendor when Cloudflare already covers our needs. Switching frameworks for a small launch-blocking site costs more than it pays.

**Plain HTML/CSS/JS on Cloudflare Pages.** Was the prior `docs/archive/website-spec.md` direction. Rejected because the signup form's server-endpoint requirement (calling the wrapper from server-side, not client-side, for the `WRAPPER_SIGNUP_SECRET` if used) needs a build/runtime layer. Adding SvelteKit later is more friction than starting with it.

**SvelteKit on Vercel.** Same framework, different host. Rejected on hosting grounds: Cloudflare Pages is free at our scale, edge CDN comes by default, and we're not on Vercel for any other surface. No reason to add a vendor.

**SvelteKit on Netlify.** Functionally equivalent to Cloudflare Pages for this use case. Rejected as a tie-breaker; Cloudflare's analytics integration and DNS proximity (paneapp.ai's DNS is Cloudflare-managed) made Cloudflare Pages the cleaner pick. Netlify would also work.

**SvelteKit self-hosted (e.g., Fly.io, Railway).** Maximum control. Rejected because a launch-blocking marketing site with no special hosting needs gains nothing from self-hosting — and adds operational overhead (deploy pipelines, scaling, monitoring).

**Wrapper proxies all paneapp.ai signups (paneapp.ai is pure static).** Considered: paneapp.ai is plain HTML/CSS/JS, the signup form POSTs directly to the wrapper from the browser. Rejected because (a) directly exposing the wrapper signup endpoint to all browser-origin traffic broadens the attack surface vs. paneapp.ai-server-mediated calls, (b) the SvelteKit server-endpoint approach lets us add a shared secret check the browser couldn't carry, and (c) the discipline of "frontend talks to wrapper through a server endpoint, never directly" matches how the wrapper is architected. Marginal cost; meaningful benefit.

**Supabase RPC + `SECURITY DEFINER` for `user_lookup` write, Supabase public `auth.signUp()` for the user.** Considered as the "no new wrapper endpoint" path. Rejected for reasons in §Rationale: scatters the ecom-write boundary, complicates rate limiting, weakens the wrapper-as-chokepoint architecture per ADR-0028.

## References

- ADR-0027 — Pane↔Ecom API Contract (the five-endpoint shape this ADR extends)
- ADR-0028 — Ecom Platform Selection (Supabase + Stripe + thin wrapper)
- ADR-0030 — paneapp.ai Stripe Return-URL Contract
- ADR-0031 — paneapp.ai Signup Flow
- ADR-0023 — External Contract Capture Discipline
- Site spec: `docs/spec/paneapp-ai.md`
- Implementation issue: `pane-ecom-backend#13` (paneapp.ai build, launch-blocking)
- Coordination item: `pane-ecom-backend#2` (wrapper umbrella) — needs sub-task for `POST /v1/auth/signup` endpoint
- Archived prior decision (informally superseded): `docs/archive/website-spec.md`
- Cloudflare Pages limits: https://developers.cloudflare.com/pages/platform/limits/
- SvelteKit Cloudflare adapter: https://kit.svelte.dev/docs/adapter-cloudflare
- Sub-tasks named by this ADR:
  1. Extend Reed PR-review coverage to the new `paneapp-ai` repo
  2. File new wrapper endpoint coordination item under `pane-ecom-backend#2`: `POST /v1/auth/signup` contract design
  3. File capture re-verification cadence sub-task for paneapp.ai's third-party surfaces (inherits from ADR-0028's existing cadence)
