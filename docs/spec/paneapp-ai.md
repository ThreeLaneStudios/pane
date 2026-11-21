# paneapp.ai — Site Specification

**Last Updated:** 2026-05-10
**Status:** SPEC LOCKED — implementation tracked in `pane-ecom-backend#13` (launch-blocking).
**Scope:** Customer-facing website for Pane. Three load-bearing surfaces: marketing, signup, Stripe return pages.

---

## §1 Scope

paneapp.ai v1 has exactly three responsibilities:

1. **Marketing surface** — convince a prospect to subscribe. Single-page minimum.
2. **Signup flow** — collect email + password, create a Supabase Auth account, redirect to Stripe Checkout. Per ADR-0031.
3. **Stripe return pages** — three brochure-style pages Stripe redirects to after Customer Portal or Checkout flows. Per ADR-0030.

Plus one supporting surface: the Pane DMG download (per §6).

That is the complete v1 product. Anything not on this list is §8 (Out of scope).

The site is **launch-blocking**: without it, no customer can sign up, and Stripe recovery flows (`subscription_action_url` per ADR-0029) dead-end.

The site is **wrapper-coordinated, not wrapper-replicated**: paneapp.ai does not own subscription state, entitlement state, or active-machine state. It writes a Supabase Auth user (and a `user_lookup` row) at signup, and after that all state mutation flows through Stripe → wrapper webhooks → Supabase `app_metadata`. paneapp.ai pages are display-only after-the-fact.

---

## §2 Signup flow

### Customer flow

The ordered customer flow is locked in **ADR-0031 §Decision steps 1–9** (Option A: account creation before Checkout). This section adds the paneapp.ai-side specifics ADR-0031 doesn't pin.

### Form fields and validation

The signup form collects two fields:

| Field | Validation |
|---|---|
| `email` | RFC 5322 format, max 254 chars, lowercase-normalize before submit |
| `password` | Min 8 chars, at least one letter and one digit. No upper-bound enforcement (passphrase-friendly) |

Match Supabase Auth's defaults rather than imposing stricter rules. Customers using password managers will not be punished by Pane's policy. Password strength meters are out of scope for v1.

Email is the primary key for the account and Stripe customer; correctness matters more than password strength. Inline validation runs on blur for email format; password validation runs on submit only (no live strength feedback).

### Email-already-exists collision

ADR-0031 §Negative flags this as a real failure mode and ADR-0031 §Mitigation pins the recovery shape: "two recovery paths." paneapp.ai catches the Supabase 422 / `user_already_exists` response and presents:

```
This email already has a Pane account.

[Sign in instead →]               (links to the existing-account sign-in surface)
[Pick up where you left off →]    (routes the existing account to a Checkout-only flow)
```

The "Pick up where you left off" path covers the half-account case (Supabase user exists, no `stripe_customer_id`) — ADR-0031 §Negative names this. Implementation: paneapp.ai calls a wrapper endpoint that creates a fresh Stripe Checkout Session for the existing `supabase_user_id` (no new Supabase user). The customer enters payment, the existing account becomes paid, normal flow resumes.

The "Sign in instead" path is for customers who completed signup previously and forgot. In v1 this links to a paneapp.ai sign-in surface that authenticates against Supabase Auth (via `auth.signInWithPassword`) and, on success, surfaces an account-status page (e.g., "You're subscribed — open Pane to continue" or "Your subscription needs renewal — [Resubscribe →]"). The sign-in surface is a paneapp.ai page; it is not Pane's S1a/S1b in-app flow.

> **Open call for the build session.** The "Sign in instead" recovery path adds a paneapp.ai-side sign-in surface that doesn't otherwise exist in v1. If this expands scope unacceptably, an alternative is to land only the "Pick up where you left off" path in v1 and route "Sign in instead" to a `mailto:` or support contact. Recommend full sign-in surface; flagged for confirmation.

### Email verification timing

**Disabled in v1.** Stripe payment is sufficient identity proof — paying customers are real customers, and the friction of email verification between payment and first sign-in to Pane (per ADR-0031 §Rationale) is exactly what Option A optimizes against.

This is ADR-worthy. See companion ADR (paneapp.ai email verification posture).

Implementation: Supabase Auth project setting "Confirm email" → off. New users are created with `email_confirmed_at` set immediately on signup.

Future: if abuse signals emerge post-launch (synthetic accounts, throwaway emails on free trials, etc.), revisit. v1 has no free trial — Stripe takes payment at signup — so the abuse vector is bounded.

### Loading states

Between form submit and Stripe Checkout redirect, the customer waits ~1–2 seconds for two sequential operations: Supabase user-create, then Stripe Checkout Session create. paneapp.ai must show a clear loading state during this window.

| State | UI |
|---|---|
| Form valid, submitted | Submit button → spinner, button disabled, form fields disabled |
| Supabase user-create in flight | Same as above, no copy change |
| Supabase user-create succeeded, redirecting to Stripe | "Redirecting to checkout…" copy under the button |
| Supabase user-create failed (network) | Inline error: "Couldn't create your account. Try again." Form re-enabled. |
| Supabase user-create failed (email-already-exists) | Inline error per §"Email-already-exists collision" above |
| Supabase user-create succeeded, Stripe Checkout Session create failed | Inline error: "Couldn't reach Stripe. Try again in a moment." Form re-enabled. Note: account exists at this point (half-account); customer retry will hit email-already-exists. The recovery path resolves it. |

### Stripe Checkout integration shape

**Hosted Checkout page redirect**, not Embedded Checkout / modal.

Why hosted:
- Simplest integration (single API call to create Session, single redirect)
- Stripe-hosted, so PCI scope stays minimal (no card data ever touches paneapp.ai)
- Brand consistency with the customer's expectation (Stripe's checkout UI is industry-standard; customers trust it)
- Mobile-friendly by default (Stripe handles responsive layout)
- Cleanest return-URL contract — Stripe redirects to `/subscribe/success` on success and `/subscribe/cancel` on abandonment, exactly matching ADR-0030

Embedded Checkout was considered. Rejected because (a) it requires more glue (mounting an iframe, handling embedded events), (b) it ties paneapp.ai's layout to Stripe's iframe sizing, and (c) the brand-consistency benefit of "stays on paneapp.ai" is mostly cosmetic given the customer is already on a payment surface.

### `app_metadata` write surface

What paneapp.ai writes vs. what the wrapper writes (post-webhook):

| Field | Written by | When |
|---|---|---|
| Supabase Auth user (`auth.users` row) | paneapp.ai | Signup form submit (ADR-0031 step 3) |
| `ecom.user_lookup` row (`supabase_user_id`, `email`, no `stripe_customer_id` yet) | paneapp.ai | Signup form submit (ADR-0031 step 4) |
| `app_metadata.stripe_customer_id` | wrapper (via webhook) | `checkout.session.completed` (ADR-0031 amended step 7a) |
| `app_metadata.subscription_status` | wrapper (via webhook) | `customer.subscription.created` (ADR-0031 amended step 7b), then ongoing |
| `app_metadata.subscription_status` | wrapper (via signin fallback) | Signin-time Stripe API query when `stripe_customer_id` present + `subscription_status` absent (ADR-0031 §Race Window, #1144) |
| `ecom.user_lookup.stripe_customer_id` | wrapper (via webhook) | `checkout.session.completed` (ADR-0031 amended step 7a) |

paneapp.ai writes only at the moment of signup. After redirect to Stripe Checkout, paneapp.ai stops mutating state. All subsequent state changes flow through Stripe webhooks → wrapper.

### Service role boundary

**paneapp.ai must not hold the Supabase service role key.** Service role gives admin write access to any user's `app_metadata` — a key compromise is a full ecom takeover.

Two implementation options:

1. **Wrapper-mediated signup (recommended).** paneapp.ai POSTs to a new wrapper endpoint `POST /v1/auth/signup` (anonymous, like `/v1/auth/email-check`). The wrapper holds service role; performs both writes (Supabase Auth user + `ecom.user_lookup` row) atomically server-side; returns `200` on success or specific error codes for the recovery paths. This is the symmetric extension of the existing five-endpoint contract (ADR-0027) and keeps service role inside the wrapper.

2. **Direct Supabase signup + RPC for `user_lookup`.** paneapp.ai uses the public `auth.signUp()` (anon key only) to create the Supabase user, then calls a Supabase RPC function with `SECURITY DEFINER` that writes the `user_lookup` row scoped to the newly authenticated user.

Recommend option 1 — wrapper-mediated. Cleanest split (paneapp.ai is pure frontend; wrapper owns ecom writes), symmetric with the existing wire contract, and gives the wrapper a natural place to enforce signup-side rate limiting (a real concern since this endpoint creates Stripe-billable customers).

This adds a new wrapper endpoint outside ADR-0027's five-endpoint shape. Filed as a coordination item with `pane-ecom-backend#2` (the wrapper umbrella). The contract for this endpoint should land in a wrapper-side spec-prep capture before paneapp.ai implementation begins.

---

## §3 Return pages

The functional contract for the three pages is locked in:

- **ADR-0030** (env var assignments, "must convey" / "must not" per page)
- **`docs/spec-prep/2026-05-09-paneapp-ai-stripe-return-pages.md`** (sibling-service contract, define-not-capture)

This section adds paneapp.ai-side specifics not in those documents.

### URL paths and Stripe primitives (recap from ADR-0030)

| Path | Stripe env var | Triggered by |
|---|---|---|
| `/account` | `STRIPE_PORTAL_RETURN_URL` | Customer returns from Stripe Customer Portal (S7 hard-block, grace-period soft-block) |
| `/subscribe/success` | `STRIPE_CHECKOUT_SUCCESS_URL` | Customer completes a Stripe Checkout Session (first signup OR re-subscribe after `canceled`) |
| `/subscribe/cancel` | `STRIPE_CHECKOUT_CANCEL_URL` | Customer abandons a Stripe Checkout Session |

### `/subscribe/success` carries the DMG download CTA

Per Matt's reading of ADR-0030 (interpretation 2 of the drift between ADR-0030 and ADR-0031 step 9): a static download CTA on `/subscribe/success` does not violate ADR-0030's "Must not make API calls / store tokens / verify subscription state" — a download link is just a hyperlink to a static asset, not state mutation.

The page therefore carries:

- The locked "informational" copy per ADR-0030 ("Subscription active. Open Pane to continue.")
- A prominent **Download Pane for Mac** CTA pointing at the paneapp.ai-hosted DMG (per §6)

The page does not distinguish first-time signup vs. re-subscribe-after-canceled (per spec-prep §2). The same page, copy, and CTA serve both. A customer who already has Pane installed simply ignores the download CTA.

### `/subscribe/cancel` recovery framing

Per ADR-0030: "Must not treat abandonment as an error." The page carries a soft retry path:

- "No changes were made to your account."
- "You can subscribe anytime from within Pane, or [start over →] here." (linking to the signup form)

The "start over" link routes back to paneapp.ai's signup form. Customers in this state have a `auth.users` row but no `stripe_customer_id` (half-account per ADR-0031 §Negative). On re-attempt they hit email-already-exists; the "Pick up where you left off" path resolves it.

### `/account` (Portal return)

Strictly per ADR-0030. Single-screen acknowledgment, no download CTA (these customers already have Pane), no Pane-launch deep-link in v1 (deep-link handling is post-launch — see §8). Copy: "Changes saved. Return to Pane to continue."

### Telemetry on return pages

**No telemetry on return pages in v1.** Justification:

- ADR-0030 explicitly forbids API calls from these pages
- The wrapper's webhook logs already capture all relevant events (Checkout completed, Portal closed, subscription state changes)
- Customer-funnel analytics (which page a customer landed on, time-on-page, etc.) is post-launch concern (§8)
- Anonymous page-view counts via Cloudflare Web Analytics (privacy-respecting, no JS injected) are acceptable site-wide and don't constitute "API calls" — but they are out of scope for v1 and tracked as a post-launch enhancement

Make this call explicit so future scope creep ("can we just add a small pixel for conversion tracking?") has a documented baseline to push back against.

---

## §4 Marketing surface

The marketing surface is a single landing page at `/`. v1 contains the minimum content to convert a qualified visitor (someone who came from a referral, blog post, or social mention and wants to know "what is this and should I pay for it").

### Required content slots

What goes on the page (the "what to say" frame). Specific copy, headings, and visual layout are the design brief's deliverable.

1. **Hero** — One-line value proposition + sub-line + primary CTA ("Get Pane →" routing to signup). The hero must answer "what is Pane in 5 seconds."

2. **Three-to-five feature highlights** — What Pane does, framed for the prospect. Bias toward concrete capabilities (sign-in, AI, OpenClaw) over abstract claims ("the future of"). Match the design language Pane already uses internally so a customer who tries the app finds it consistent.

3. **Pricing display** — Per §5. Inline on the landing page, not a separate `/pricing` page in v1.

4. **Secondary CTA at page bottom** — "Get Pane →" again, routing to the same signup form. Page-bottom CTA captures readers who scroll without converting on the hero.

### What's deliberately not on v1

- Testimonials (no customers yet)
- Logos / "as featured in" (none yet)
- Comparison tables vs. competitors
- FAQ section
- About / team page
- Demo video or interactive product tour
- Email-capture (newsletter) — separate concern, post-launch

These are post-launch additions when there's something real to show.

---

## §5 Pricing display

**$29/month, USD, billed monthly.** Single tier. No annual discount in v1, no team plans, no enterprise.

### Where displayed

- Inline on the landing page (per §4)
- On the signup form as a confirmation: "You'll be charged $29/month after signup."
- In Stripe Checkout (Stripe sources from the Stripe Price object — no paneapp.ai responsibility)

### Single source of truth

**Hardcoded in paneapp.ai code in v1**, not pulled from the Stripe Price API at render time.

Reasons:
- Simplicity: no API call on landing page render, no caching layer, no failure mode when Stripe is slow
- Stability: pricing changes are infrequent and intentional; a code-deploy is the right ceremony for them
- Consistency: the Stripe Price object is the source of truth for the actual charge; a deliberate code change to paneapp.ai is the source of truth for the marketing display. These should match by convention, not by API coupling

Implementation: a single TypeScript constant (e.g., `PRICE_USD_CENTS = 2900`, `PRICE_DISPLAY = "$29/month"`) imported wherever displayed. Comment in code references the corresponding Stripe Price ID for cross-check.

If pricing changes: update the constant in paneapp.ai, deploy, and ensure the Stripe Price object matches (or update Stripe's `STRIPE_PRICE_ID` env var on the wrapper to point at a new Price). Two coordinated changes; the discipline matches the gravity of pricing changes.

Future: a "pricing config" pulled from the wrapper at deploy time is post-launch concern. Hardcoding is the correct v1 cut.

---

## §6 DMG distribution

### Hosting

**paneapp.ai-hosted via Cloudflare Pages.** The DMG ships as a static asset alongside the rest of the site, under the same Cloudflare Pages deployment as the marketing pages.

Rationale:
- **Single infrastructure, single URL.** Cloudflare Pages already serves the site; the DMG is just one more file in the deployment.
- **Free CDN.** Cloudflare Pages serves from edge worldwide; the customer's first launch download benefits from CDN caching.
- **Brand consistency.** Customer sees `paneapp.ai/Pane.dmg` in the download bar — never a third-party domain (`github.com`, S3, etc.).
- **No third-party dependency.** GitHub Releases works but introduces a dependency on github.com being up at customer-download-time; Cloudflare Pages is a single domain to monitor.

Cloudflare Pages limits the team should know:
- Single-file limit: 25 MB per file
- Pane DMG size as of v0.65.18: ~11 MB (well under limit)
- Total deployment size: 25 GB (irrelevant for this scale)
- Total file count: 20,000 (irrelevant)

If the DMG ever approaches 25 MB, switch to Cloudflare R2 + custom domain for the asset (post-launch, document at the time).

### Asset path

**`https://paneapp.ai/Pane.dmg`** — always points at the latest released version. No version-suffixed path in v1; the customer always gets the current release.

Update mechanism: a deploy of paneapp.ai with the new DMG file replaces the previous one. Cloudflare Pages handles cache invalidation automatically on deploy.

Versioned URLs (e.g., `/Pane-v0.65.18.dmg`) are post-launch concern. v1 customers always get latest.

### Three surfaces

The DMG is reachable from three places:

1. **`/subscribe/success`** — prominent download CTA. The most-used path: customer just paid, expects the DMG one click away.
2. **Stripe receipt email** — Stripe's automatic receipt includes a `download_url` custom field set by the wrapper at Checkout Session creation, pointing at `paneapp.ai/Pane.dmg`. The customer can find it later if they don't download immediately.
3. **`/download` page** — a public page at `paneapp.ai/download` for organic visitors (referral traffic, future link-shares, customers re-installing on a new machine). The page is a thin landing page: brief copy, download CTA, system requirements (macOS version), nothing else.

Belt-and-suspenders. Three surfaces; one underlying URL.

### Why public `/download` is fine

`/download` is reachable without subscription. This is acceptable because:
- Pane gates access at sign-in, not at install. A non-subscriber who downloads the DMG and runs it lands at S1a, enters an email, hits S1c "no account found → Get Pane" which routes them to paneapp.ai signup. The DMG itself is not the gate.
- Public download is the right behavior for a customer reinstalling on a new machine — they should not be made to authenticate before downloading.
- The brand benefit of "anyone can grab the app" is real; the abuse risk is bounded by the in-app gate.

### Future considerations

- Auto-update infrastructure (Tauri's updater, Sparkle, etc.) is post-launch (`#536` Tier 2). v1 customers update by manually downloading a new DMG.
- Code signing and notarization status of the DMG must be verified for every release. Out of scope for this spec; the build pipeline owns it.
- Windows / Linux DMGs (or installers) are post-launch.

This decision is ADR-worthy. See companion ADR (paneapp.ai DMG distribution).

---

## §7 Stack and deployment

### Framework

**SvelteKit (latest stable, currently 2.x).**

Rationale:
- Matches Pane's frontend stack (Svelte/SvelteKit). Brit and Stia already operate in this idiom; no new framework to onboard onto.
- SvelteKit's hybrid model (mostly static, server endpoints where needed) fits paneapp.ai's shape exactly: marketing pages and return pages are static; the signup form needs one server endpoint to call the wrapper.
- First-class Cloudflare Pages adapter (`@sveltejs/adapter-cloudflare`).
- TypeScript-first; matches Pane's frontend.

Alternatives considered:
- **Astro + Svelte islands** — best-in-class for content-heavy sites, but paneapp.ai is small enough that the islands architecture is unneeded ceremony. SvelteKit's static rendering is sufficient.
- **Next.js** — biggest ecosystem, but Brit and Stia don't operate in React idiomatically. Switching frameworks for a small site costs more than it pays.
- **Plain HTML/CSS/JS** — was the prior `docs/archive/website-spec.md` direction. Insufficient for the signup form's wrapper-endpoint call; introducing a build tool partway is more friction than starting with SvelteKit.

### Hosting

**Cloudflare Pages.**

Rationale:
- Free tier covers v1 traffic comfortably (unmetered bandwidth on free tier)
- Edge CDN by default
- GitHub-native deploys (push to main → auto-deploy; PR → preview deployment)
- Cloudflare Workers / Pages Functions support if a server-side endpoint is needed (yes — see §2 service role boundary)
- Same vendor as paneapp.ai's analytics path (Cloudflare Web Analytics) and DNS (assumed Cloudflare DNS — verify pre-deploy)
- Continuous with the prior archived `website-spec.md` choice

Deployment:
- GitHub Action triggered by push to `main` branch in the new `paneapp-ai` repo
- Branch previews auto-generated for PRs (Cloudflare Pages native)
- Production domain: `paneapp.ai` (apex) and `www.paneapp.ai` (redirect to apex)

### Environment variables

paneapp.ai needs these env vars at build/deploy time:

| Var | Purpose | Type |
|---|---|---|
| `PUBLIC_SUPABASE_URL` | Supabase project URL | Public |
| `PUBLIC_SUPABASE_ANON_KEY` | Supabase anon key (client-safe) | Public |
| `PUBLIC_STRIPE_PUBLISHABLE_KEY` | Stripe publishable key (client-safe) | Public |
| `PUBLIC_PANE_DMG_URL` | Path to DMG (e.g., `/Pane.dmg`) | Public |
| `PUBLIC_WRAPPER_BASE_URL` | Wrapper base URL. Production: `https://api.paneapp.ai`. Staging: `https://api-staging.paneapp.ai`. | Public |
| `WRAPPER_SIGNUP_SECRET` | Optional shared secret for paneapp.ai → wrapper signup endpoint, if added | Server-only |

Notably absent: `SUPABASE_SERVICE_ROLE_KEY`. paneapp.ai must never hold service role per §2 service role boundary.

If `WRAPPER_SIGNUP_SECRET` is used, it lives only in Cloudflare Pages' encrypted env-var store, accessible only inside SvelteKit server endpoints (not exposed to client bundles). The wrapper validates the shared secret on receipt as a coarse anti-abuse measure (cheap to add; doesn't replace rate limiting).

### Libraries

| Library | Purpose |
|---|---|
| `@sveltejs/kit` | Framework |
| `@sveltejs/adapter-cloudflare` | Cloudflare Pages deploy target |
| `@supabase/supabase-js` | Supabase Auth client (sign-in surface; signup goes via wrapper) |
| `@stripe/stripe-js` | Stripe.js client for Checkout redirect |

Tailwind CSS for styling — matches the design-token approach the design brief will land. Lighter alternatives exist; Tailwind is the path of least resistance and has good Cloudflare Pages tutorials.

### CI

Per the dev-hygiene playbook gate shape (lint + build + deploy preview):

- **Lint:** `eslint`, `prettier --check`, `svelte-check` (TypeScript)
- **Build:** `npm run build` (must succeed)
- **Deploy preview:** Cloudflare Pages branch preview (automatic on PR)
- **No production tests required for v1.** End-to-end Stripe testing happens via `pane-ecom-backend#12` (Stripe test-mode E2E dry-run). paneapp.ai's role in that test is the surface-under-test, not the test driver.

Post-launch: add Playwright tests for the signup form happy path and the email-already-exists recovery path. Sub-task to file at launch.

This decision is ADR-worthy. See companion ADR (paneapp.ai stack and hosting).

---

## §8 Out of scope for v1

Explicit list. Future scope creep gets pushed back against this list:

- **Blog** (`/blog`, MDsveX, content marketing)
- **Documentation site** (`/docs`, customer-facing help content)
- **Support portal** (`/support`, ticketing, FAQ)
- **Multi-language / i18n** — English only in v1
- **Free trial flow** — Stripe takes payment at signup; no trial in v1
- **Team / Pro / Enterprise plans** — single-tier $29/month only
- **Annual billing discount** — monthly only
- **Account self-management UI on paneapp.ai** — Stripe Customer Portal handles billing self-service; nothing else needs a paneapp.ai surface
- **In-app account dashboard / device management page on paneapp.ai** — Pane handles its own state, not paneapp.ai
- **Analytics dashboard for the customer**
- **Newsletter signup / email capture** — post-launch, when there's content to send
- **Comparison page vs. competitors**
- **Testimonials / social proof**
- **Demo video or interactive product tour**
- **Auto-update for the desktop app** (Tauri updater is post-launch)
- **Versioned DMG URLs** (e.g., `/Pane-v0.65.18.dmg`) — v1 always serves latest
- **Windows / Linux installers** — macOS DMG only in v1
- **Deep-linking from `/account` to launch Pane** (custom URL scheme `pane://`) — post-launch
- **Cookie banner / consent management** — Cloudflare Web Analytics is cookieless by default; if Plausible or anything stateful is added later, a banner becomes needed
- **A/B testing infrastructure**
- **Customer-facing changelog**
- **Conversion tracking pixels / ad-network integrations**
- **Sign-up via SSO (Google / GitHub / Apple)** — email + password only in v1

---

## References

- ADR-0030 — paneapp.ai Stripe return-URL contract
- ADR-0031 — paneapp.ai signup flow (account creation before Stripe Checkout)
- ADR-0028 — Ecom Platform Selection (Supabase + Stripe + thin wrapper)
- ADR-0029 — Entitlement Response Shape and Email-Check Privacy Posture
- ADR-0027 — Pane↔Ecom API Contract
- ADR-0023 — External Contract Capture Discipline
- `docs/spec-prep/2026-05-09-paneapp-ai-stripe-return-pages.md` — return-page sibling-service contract
- `docs/spec-prep/2026-05-07-pane-ecom-contract.md` — Pane↔wrapper contract
- `docs/spec-prep/2026-05-07-supabase-auth-platform-capture.md` — Supabase Auth third-party capture
- `docs/spec-prep/2026-05-07-stripe-platform-capture.md` — Stripe third-party capture
- `docs/dev-hygiene-playbook.md` §1 (external-contract capture), §2 (ADR immutability), §4 (per-entry-point coverage)
- `pane-ecom-backend#13` — paneapp.ai build issue (launch-blocking)
- `pane-ecom-backend#12` — Stripe test-mode E2E dry-run (blocked on #13)
- `pane-ecom-backend#2` — wrapper umbrella issue
- Companion ADRs filed with this spec:
  - paneapp.ai stack and hosting (SvelteKit + Cloudflare Pages)
  - paneapp.ai DMG distribution (paneapp.ai-hosted, three surfaces)
  - paneapp.ai email verification posture (disabled, Stripe-as-identity)
- Companion design brief: `docs/design-briefs/paneapp-ai.md`
