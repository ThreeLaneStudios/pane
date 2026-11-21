# ADR-0031: paneapp.ai signup flow — account creation before Stripe Checkout

**Status:** Accepted  
**Date:** 2026-05-10  
**Amended:** 2026-05-16 — clarifying amendment pinning `client_reference_id` mechanism for customer ↔ Supabase link. Steps 4, 7, and §Mitigation paragraph updated. No supersession.
**Amended:** 2026-05-17 — added §Race Window between webhook events 7a and 7b. Documents the wrapper signin Stripe API fallback (#1144) that closes the customer-facing race during webhook delivery. No supersession.  
**Supplements:** ADR-0028 (Ecom Platform — Supabase + Stripe + thin wrapper), ADR-0030 (paneapp.ai Stripe Return-URL Contract)  
**Related:** ADR-0027 (Pane↔ecom API contract), ADR-0029 (Entitlement response shape)

## Context

paneapp.ai's signup flow needs to do two things: create a Supabase Auth account (email + password) and create a Stripe subscription via Checkout. The order of these two steps is a design decision with downstream implications for wrapper integration, customer experience, and abandoned-signup handling.

Two viable orderings:

**Option A — Account creation before Checkout.** Customer enters email + password on paneapp.ai; Supabase Auth account is created; customer is redirected to Stripe Checkout; on successful payment, subscription is provisioned. The Supabase account exists from the moment of form submission, regardless of whether Checkout completes.

**Option B — Account creation after Checkout.** Customer enters email only on paneapp.ai (or directly in Stripe Checkout); customer pays via Stripe Checkout; Stripe webhook fires; wrapper creates the Supabase account post-payment; customer receives an email with a password-set link. The Supabase account exists only after successful payment.

The two orderings produce structurally different states and require different recovery paths.

## Decision

paneapp.ai uses **Option A — account creation before Stripe Checkout**.

### Customer flow:

1. Customer arrives at paneapp.ai, clicks "Get Pane."
2. Customer enters email + password on a paneapp.ai form.
3. paneapp.ai calls Supabase Auth to create the account. Account is now in Supabase with no `stripe_customer_id` in `app_metadata`.
4. paneapp.ai writes a row to `ecom.user_lookup` with the new `supabase_user_id` (`stripe_customer_id` left null until step 7). This avoids the wrapper falling back to paginated `listUsers()` on the customer's first email-check from Pane. The Stripe Checkout Session created for this signup has `client_reference_id` set to the new `supabase_user_id`. This is the mechanism by which the webhook later resolves the Stripe customer back to the Supabase user (see amended step 7).
5. Customer is redirected to Stripe Checkout.
6. Customer completes payment.
7. Two Stripe webhook events fire after successful payment:
   a. `checkout.session.completed` fires first. Wrapper extracts `client_reference_id` (the `supabase_user_id` set at session creation per step 4) and `session.customer` (the Stripe customer ID). Wrapper writes `stripe_customer_id` to `app_metadata` and updates the `user_lookup` row with `stripe_customer_id`. The customer ↔ Supabase link is now established.
   b. `customer.subscription.created` fires shortly after. Wrapper resolves Stripe customer ID → `supabase_user_id` via the link just written (`user_lookup` indexed lookup), and writes `subscription_status: active` to `app_metadata`.
8. Customer is redirected to `/subscribe/success` per ADR-0030.
9. Customer downloads the Pane DMG, installs, signs in. The account already exists, so S1a email-check returns `true` immediately and the customer proceeds straight to S1b. (See §Race Window below for the edge case where signin races webhook delivery.)

## Rationale

**Simpler customer experience.** Option A matches the customer's mental model: "I just paid, I have an account, I can sign in." Option B requires a second step (check email, click link, set password) between payment and first use, which adds friction at the highest-stakes moment of the customer journey.

**Email captured for marketing on abandonment.** When a customer reaches step 3 but doesn't complete step 6, their email is in Supabase. Future marketing flows (post-launch) can re-engage these customers — abandoned-checkout email, retention nudges, eventual relaunch announcements. Option B forfeits this; until payment completes, there is no email anywhere.

**Industry standard for paid SaaS.** Most subscription products with self-serve signup use Option A. Customers expect this shape.

**Pane sign-in already assumes account-exists-on-arrival.** The S1a → S1b → S1c flow per `docs/spec/onboarding.md` §6.1.1 is built around `POST /v1/auth/email-check` returning `true` for known accounts. With Option A, every paying customer's email returns `true` from the moment they finish the paneapp.ai form — independent of payment timing. Option B would mean a window where a paid customer's email-check returns `false` because the wrapper hasn't yet processed the webhook that creates the account, putting the customer at S1c (no-account-found) despite having paid moments earlier.

**`user_lookup` population happens at the right moment.** Per `pane-ecom-backend#10`, the wrapper's `checkEmailExists` and webhook `user-lookup` paths fall back to a paginated `listUsers()` when `ecom.user_lookup` doesn't have the row. Option A lets paneapp.ai write to `user_lookup` at signup time — so the customer's first sign-in from Pane hits the indexed lookup, not the fallback. Option B has no clean point for paneapp.ai to populate `user_lookup`; the wrapper would need to do it post-webhook, after `listUsers()` already missed at least once.

## Consequences

### Positive:

- Simplest possible customer flow from payment to first use.
- Email captured for abandoned-checkout marketing.
- `user_lookup` is populated at the customer's earliest possible touchpoint, removing a `listUsers()` fallback from the hot path.
- Sign-in flow assumptions in `docs/spec/onboarding.md` §6.1.1 hold without modification.

### Negative:

- **Half-accounts.** Customers who reach step 3 (Supabase account created) but abandon at step 5 or 6 (don't complete Checkout) leave a Supabase account with no `stripe_customer_id` and no subscription. These accounts are not actively harmful but accumulate over time.
- **Email-already-exists collision on retry.** A customer who abandoned a previous signup attempt and tries again will hit "email already registered" on step 3. paneapp.ai must handle this gracefully — either offer a sign-in path with the existing password, or a "pick up where you left off" Checkout-only path.
- **Email verification timing.** Supabase Auth defaults to requiring email verification before sign-in. paneapp.ai must either disable email verification (acceptable trade for paid customers, given Stripe verifies them via payment) or surface the verification step explicitly. This is a paneapp.ai-side decision with implications for the customer experience between payment and first sign-in.

### Mitigation for half-accounts

A nightly cleanup job deletes Supabase Auth users that meet all of:

- Created more than 24 hours ago
- `app_metadata.stripe_customer_id` is null
- No corresponding row in `ecom.user_lookup` with a non-null `stripe_customer_id`

This is a wrapper-side cron job, post-launch. Tracked as a follow-up issue. Until the cron lands, half-accounts accumulate but don't break anything — wrapper's email-check returns `true` for them, sign-in returns `subscription_status: canceled`, and the customer hits S7 with a Resubscribe CTA. The functional path is correct; the database is just messier than ideal. The Stripe Checkout Session created for S7 recovery threads `client_reference_id: supabase_user_id` identically to the happy path; the webhook resolves the link via `checkout.session.completed` per amended step 7. The S7 recovery path therefore uses the same mechanism as the paneapp.ai happy path, not a separate one. Distinct from §Race Window below, which addresses a *paid* customer whose `subscription_status` hasn't yet landed via webhook.

### Race Window between webhook events 7a and 7b

Between `checkout.session.completed` (step 7a) and `customer.subscription.created` (step 7b), a customer who signs in fast may see `stripe_customer_id` present in `app_metadata` but `subscription_status` absent. Without mitigation, the wrapper's signin coerces missing status to `canceled` and the customer hits S7 Resubscribe despite having paid.

Wrapper signin handles this race with a Stripe API fallback (implemented in #1144). When `stripe_customer_id` is present but `subscription_status` is missing, the wrapper queries Stripe directly via `stripe.subscriptions.list({customer: stripe_customer_id, status: 'all', limit: 1})`, maps the result through existing status mapping, and writes the resolved status back to `app_metadata` so subsequent signins don't repeat the call. The race closes permanently for that user on first signin during the window.

If the Stripe API call fails (network error, Stripe outage), the wrapper falls through to the `canceled` coercion — customer hits S7 Resubscribe (degraded but functional), and the webhook will land on a subsequent request. Log the Stripe API error for diagnostic visibility.

The fallback fires only when `stripe_customer_id` is present + `subscription_status` is absent. It does NOT fire when both are absent (half-account per §Mitigation — correctly routes to S7) or when both are present (normal path — uses `app_metadata` directly).

### Mitigation for email-already-exists

paneapp.ai's signup form catches the Supabase error and offers two recovery paths: (a) "You already have an account — sign in here" (link to sign-in surface) or (b) "Pick up where you left off" (route to Stripe Checkout for the existing account). Implementation detail for paneapp.ai; called out here so the build session designs for it.

## Alternatives considered

**Option B — Account creation after Checkout** was rejected for the reasons in §Rationale: extra customer friction at the worst possible moment, no email captured on abandonment, race window between webhook delivery and customer sign-in attempt, and no clean point for `user_lookup` population.

**Hybrid — Email captured on paneapp.ai, account created by wrapper post-payment.** A variant of Option B where paneapp.ai writes `(email, signup_intent_at)` to a separate table on form submission, then the wrapper reconciles this at webhook time. Rejected as additional schema and reconciliation complexity for a marginal win — the half-account problem in Option A is cheaper to clean up than the reconciliation problem in this hybrid.

**Defer the marketing-email argument.** Option B with no marketing follow-up was considered. Rejected because forfeiting future marketing capability for the launch sake is a one-way door; the half-account cleanup cost in Option A is a recurring small cost we can absorb.

## References

- ADR-0028 — Ecom platform selection (Supabase + Stripe + thin wrapper)
- ADR-0030 — paneapp.ai Stripe return-URL contract
- ADR-0027 — Pane↔ecom API contract
- ADR-0029 — Entitlement response shape
- `docs/spec/onboarding.md` §6.1.1 — Pane sign-in flow assumptions
- `docs/spec-prep/2026-05-09-paneapp-ai-stripe-return-pages.md` — return-page contract
- `pane-ecom-backend#10` — `user_lookup` table and pagination fallback (closed)
- `pane-ecom-backend#13` — paneapp.ai build issue (launch-blocking)
