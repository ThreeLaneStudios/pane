# paneapp.ai — Design Brief

**For:** Claude Design session, post-spec
**From:** paneapp.ai spec session, 2026-05-10
**Spec authority:** `docs/spec/paneapp-ai.md`
**Implementation owner:** Brit + Stia (post-design)

This brief tells the design session what needs to exist, what the visual language is, and what the implementer needs back. It does not pre-write copy or layouts — those are the design session's deliverables.

---

## Pages to design

### Marketing surfaces

1. **`/` — Landing page**
   Single-page marketing surface. Hero + features + pricing + secondary CTA. Per site spec §4.

2. **`/download` — Public download page**
   Thin landing page for organic visitors and customers re-installing on a new machine. Brief copy, download CTA, system requirements (macOS minimum version), nothing else. Per site spec §6.

### Signup flow

3. **`/signup` (or signup form on `/`) — Email + password form**
   The canonical signup surface. Two fields, primary CTA, validation states, loading states. Per site spec §2.

4. **Email-already-exists recovery surface**
   Inline state on the signup form (not a separate page). Surfaces two recovery CTAs per ADR-0031 §Mitigation. Per site spec §2.

5. **`/sign-in` — Existing-account sign-in surface**
   Email + password sign-in for customers who completed signup previously. On success, surfaces an account-status page (subscribed → "open Pane" / lapsed → "resubscribe"). Per site spec §2.

6. **`/forgot-password` — Password reset request**
   Single email field, "Send reset link" CTA, confirmation state. Calls Supabase Auth's password-reset flow. Per ADR-0034.

### Stripe return pages

7. **`/subscribe/success` — Checkout success / first-install page**
   Per ADR-0030 + spec-prep `2026-05-09-paneapp-ai-stripe-return-pages.md`. Informational copy + prominent **Download Pane for Mac** CTA (ADR-0033). Same page serves first-time signup and re-subscribe-after-canceled.

8. **`/subscribe/cancel` — Checkout abandoned**
   Per ADR-0030. Soft retry framing — abandonment is not an error. Includes a "Start over" link routing to signup.

9. **`/account` — Customer Portal return**
   Per ADR-0030. Single-screen acknowledgment. No download CTA (these customers already have Pane).

### Error states

10. **Form error states across all surfaces**
    - Network failures (couldn't reach Supabase, Stripe, wrapper)
    - Validation errors (invalid email, password too short)
    - Service errors (Stripe Checkout Session creation failed)
    - Rate-limited states (signup endpoint, sign-in endpoint)
    - Half-account states (Supabase user exists, no Stripe customer — recovery CTA visible)

11. **404 / not-found page**
    Brand-consistent. Routes back to `/`.

---

## Brand context

paneapp.ai must feel of-a-piece with the Pane desktop app. A customer who lands on the website and then opens Pane should experience visual continuity, not a jarring shift.

### Design tokens (carry forward from Pane onboarding)

| Token | Value | Use |
|---|---|---|
| `--bg-base` | `#0a0a0f` | Page background |
| `--bg-surface` | `#12121a` | Cards, raised surfaces |
| `--bg-border` | `#1e1e2e` | Borders, subtle dividers |
| `--text-primary` | `#e0e0e0` | Body text, headings |
| `--text-secondary` | `#888888` | Sub-copy, helper text |
| `--accent` | `#00E4E4` | Primary CTAs, links, focus states |
| `--accent-hover` | `#00c8c8` | CTA hover state |
| `--error` | `#ff4444` | Inline errors, destructive states |
| `--radius-card` | `12px` | Cards, modals |
| `--radius-button` | `8px` | Buttons |
| `--radius-input` | `6px` | Form inputs |
| Font stack | `-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` | All text |

These match Pane's onboarding screens (S1a / S1b / S1c / S7 / S8 reference designs in project files under "Auth"). The website should look and feel like it was built by the same team — because it was.

### Visual direction

- **Clean, calm, premium.** Match the Pane onboard screen aesthetic. Not busy.
- **Dark theme primary.** Light mode is post-launch; v1 is dark only.
- **Generous whitespace.** Centered layouts on signup and return pages, mirroring Pane's onboarding modals.
- **No gradients or glow effects** unless very subtle. No animated decorations. The product should speak; the chrome should disappear.
- **Real screenshots, not stock.** Any hero imagery or feature illustrations should show actual Pane UI, dark theme. (Source: Pane app screenshots; coordinate with Brit for current-build captures.)
- **Type sets the tone.** Heading hierarchy and body weight matter more than color. Use the system stack — don't introduce a webfont.

### Component shapes already established

- Primary CTA button: full-width on signup form, inline on landing page hero, teal background (`--accent`), system font, `--radius-button`. Hover state darkens to `--accent-hover`.
- Form inputs: `--radius-input`, `--bg-surface` background, `--bg-border` 1px border, focus state shows `--accent` border. Inline error text in `--error` below the input.
- Cards (where used): `--radius-card`, `--bg-surface`, `--bg-border` 1px border. Same shape as Pane's provider-picker cards in onboarding.

The design session should not invent new component shapes for paneapp.ai. The Pane app already has a working design system; paneapp.ai inherits it. Net-new components (e.g., a pricing display block) are designed as additions to that system, not as a separate language.

---

## Content slots

What copy needs to exist on each page. The design session fills these in; this brief defines the slot, not the words.

### `/` Landing page

| Slot | Purpose |
|---|---|
| Hero headline | One-line value proposition. "What is Pane in 5 seconds." |
| Hero sub-line | One sentence elaborating the headline. |
| Hero CTA | "Get Pane →" or equivalent. Routes to signup. |
| Hero visual | App screenshot or representative imagery. Dark theme, real Pane UI. |
| Features grid | 3-5 capability highlights. Each: short heading + 1-2 sentences. |
| Pricing block | $29/month, single tier. Inline display per site spec §5. |
| Secondary CTA | Page-bottom "Get Pane →" repeat. |
| Footer | Minimal: legal links (privacy, terms — even if placeholder), copyright. |

### `/signup` form

| Slot | Purpose |
|---|---|
| Page heading | Brief — "Get Pane" or "Create your account" |
| Email field | Label + input + inline validation feedback |
| Email confirmation field | Label "Confirm email" + input + match-validation against the email field. Mitigates typo'd-email failure mode per ADR-0034 §Risks. |
| Password field | Label + input + minimum-strength hint ("8+ characters") |
| Pricing reminder | "$29/month after signup" near the CTA |
| Primary CTA | "Continue to checkout →" |
| Loading state copy | "Redirecting to checkout…" once submitted |
| Sign-in link | "Already have a Pane account? [Sign in →]" — small, secondary |
| Email-already-exists state | Two CTAs: "Sign in instead →" and "Pick up where you left off →" |
| Error states copy | Per site spec §2 loading-states table |

### `/sign-in` form

| Slot | Purpose |
|---|---|
| Page heading | "Sign in to Pane" |
| Email field | Standard |
| Password field | Standard |
| Primary CTA | "Sign in" |
| Forgot password link | "[Forgot password →]" routing to `/forgot-password` |
| Sign-up link | "Don't have a Pane account? [Get Pane →]" routing to `/signup` |
| Account status surface (post-sign-in) | "Subscribed → open Pane" or "Subscription lapsed → [Resubscribe →]" |

### `/subscribe/success`

Per ADR-0030 + spec-prep:

| Slot | Purpose |
|---|---|
| Confirmation heading | "Subscription active" / "You're all set" |
| Reassurance copy | One sentence — "Open Pane to continue" |
| Download CTA | **Prominent.** "Download Pane for Mac →" pointing at `/Pane.dmg` |
| Secondary copy | System requirements (macOS minimum), brief. |
| No links to dashboards, no API calls, no session-ID handling. | Per ADR-0030 |

### `/subscribe/cancel`

Per ADR-0030:

| Slot | Purpose |
|---|---|
| Reassurance heading | "No changes were made" |
| Soft retry copy | "You can subscribe anytime from within Pane" |
| Start-over link | "Or [start over →] here" routing to `/signup` |
| No error framing. | Per ADR-0030 |

### `/account`

Per ADR-0030:

| Slot | Purpose |
|---|---|
| Confirmation heading | "Changes saved" |
| Instruction copy | "Return to Pane to continue" |
| No deep-link to launch Pane in v1. | Per site spec §8 |

### `/download`

| Slot | Purpose |
|---|---|
| Page heading | "Download Pane" |
| Brief tagline | One line — what Pane is, for context |
| Download CTA | "Download Pane for Mac →" pointing at `/Pane.dmg` |
| System requirements | macOS minimum version, file size |
| Sign-up reminder | Small — "Need a subscription? [Get Pane →]" |

---

## Functional anchors

UI elements that are non-negotiable for functional reasons. These can change in *appearance* but not in *presence* or *placement*.

1. **Stripe Checkout submit button must be prominent on the signup form.** It's the action that turns a visitor into a customer. No buried CTAs, no two-step "review then submit" flow.

2. **Email-already-exists state must offer exactly two recovery CTAs** per ADR-0031 §Mitigation: "Sign in instead" and "Pick up where you left off." Not one, not three. Both visible at the same time.

3. **`/subscribe/success` download CTA must be the primary visual focus** below the confirmation heading. The customer just paid; the DMG should be one click away. Per ADR-0033.

4. **No telemetry pixels, conversion-tracking scripts, or analytics fragments on `/account`, `/subscribe/success`, `/subscribe/cancel`.** Per ADR-0030. Cloudflare Web Analytics (cookieless, no JS) site-wide is acceptable but is post-launch — v1 has no analytics.

5. **Loading states must be visually distinct from default and error states.** Customer must always know whether the form is processing, succeeded, or failed. Spinner alone is insufficient — paired with copy ("Redirecting to checkout…").

6. **Primary CTAs use the teal accent (`--accent`).** Secondary actions (sign-in link, forgot-password link) are text-only links in `--accent`. No competing "secondary button" treatment that creates ambiguity about which action is primary.

7. **All forms accept Enter-to-submit.** Keyboard-first usability; do not require mouse click on the CTA.

8. **Inline validation errors must appear below the relevant field, not in a global banner.** Customer scans the form top-to-bottom; errors travel with their cause.

9. **Error states must not lose customer input.** A network failure on signup does not clear the form. The customer retries with the same data.

10. **All pages must work without JavaScript except the signup form.** Marketing pages, return pages, and `/download` are static HTML/CSS, server-rendered by SvelteKit's static adapter. The signup form requires JS for validation and Stripe.js redirect; if JS fails, the form falls back to a server-side POST handler (degraded but functional).

---

## Out of scope for design

Per site spec §8 — explicitly excluded from v1 design work:

- Blog templates or article layouts
- Documentation site templates
- Support portal / FAQ pages
- Multi-language layouts / RTL support
- Free-trial UI (no free trial in v1)
- Multi-tier pricing UI (single tier only)
- Annual / monthly toggle on pricing
- Account self-management UI (Stripe Customer Portal handles this)
- Customer dashboard (no v1 dashboard)
- Newsletter signup forms
- Comparison tables vs competitors
- Testimonials or social-proof modules
- Demo videos or interactive product tours
- Cookie-consent banners (no cookies in v1)
- Light theme variants (dark only in v1)
- Mobile app download surfaces (Mac DMG only)
- Auto-update prompt UIs
- Versioned-download UIs
- A/B test variants

If the design session has strong opinions about including any of these, surface as a flag for Matt — don't ship them silently in v1.

---

## Hand-off shape — what design returns

The design session produces:

1. **A short design spec markdown doc** (`docs/design-briefs/paneapp-ai-design-output.md` or similar) covering:
   - Final copy for each content slot above
   - Component-level decisions (button states, form input states, error treatments) where they extend the existing tokens
   - Any new tokens introduced (likely: hero typography scale, marketing-page section spacing)
   - Image asset specifications (sizes, sources, alt text)
   - Annotated wireframes or static layouts for each page

2. **Static layout artifacts** — one of:
   - A simple HTML/CSS prototype committed to a `prototypes/` folder in the `paneapp-ai` repo (preferred — Brit/Stia can implement directly against it)
   - Annotated screenshots in markdown (acceptable — implementation interprets)
   - Figma file linked from the design spec doc (acceptable — but extracting tokens for code is implementer overhead)

   Recommend the HTML/CSS prototype path. Brit and Stia operate in code; static HTML/CSS is the lowest-friction handoff. SvelteKit's adapter-static can serve the prototype as a deploy preview for Matt to review before implementation begins.

3. **A copy doc covering all content slots** — could be the same markdown doc as item 1. Final copy at the moment of design hand-off, not "TBD" or "lorem ipsum."

The design session does not deliver:
- Working code (that's Brit + Stia, post-design)
- Backend integration (that's Brit + Stia + wrapper coordination)
- Marketing strategy beyond "what does the landing page need to convert"
- Pricing strategy
- Brand strategy beyond carrying forward Pane's existing visual language

---

## References

- Site spec: `docs/spec/paneapp-ai.md` (the authoritative implementation surface)
- ADR-0030 — Stripe return-URL contract (locks return-page functional behavior)
- ADR-0031 — Signup flow (locks signup ordering + email-already-exists mitigation)
- ADR-0032 — Stack and hosting (SvelteKit + Cloudflare Pages baseline)
- ADR-0033 — DMG distribution (locks download surface placement)
- ADR-0034 — Email verification posture (no verification step in v1 customer flow)
- Pane onboarding designs in project files (Auth board: S1a / S1b / S1c / S7 / S8) — visual continuity reference
- `docs/spec/onboarding.md` §6.1 — Pane sign-in flow (paneapp.ai's sign-in surface should feel adjacent)
- Frontend design tokens already established in Pane's `src/lib/styles/` (Brit can point to specific files)
