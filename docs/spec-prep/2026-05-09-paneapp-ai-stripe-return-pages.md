# paneapp.ai Stripe Return Pages — Sibling-Service Contract

**Date:** 2026-05-09  
**Type:** Define-not-capture (sibling service, same org — ADR-0023 third-party discipline does not apply)  
**Relates to:** ADR-0030 (return-URL contract), ADR-0029 (subscription_action_url), ecom-backend #13 (build paneapp.ai)

## Scope

This file defines the contract between the `pane-ecom-backend` wrapper and three pages on paneapp.ai that Stripe redirects customers to after Customer Portal or Checkout flows triggered by ADR-0029's `subscription_action_url`.

The wrapper holds the URLs as env vars; paneapp.ai is built to match. This is a sibling-service contract per ADR-0023: paneapp.ai is authored by us (same org), so the third-party-capture discipline does not apply. The format here is define-not-capture, matching `2026-05-07-pane-ecom-contract.md`.

Authoring is one-directional — this document defines the contract, whoever builds paneapp.ai reads it.

## Three pages

### 1. `/account` — Portal return

**Triggered by:** Customer returns from Stripe Customer Portal (payment method update, plan reactivation, or cancellation).

**Both Portal callsites** (S7 hard-block and grace-period soft-block) use `STRIPE_PORTAL_RETURN_URL`, which defaults to `https://paneapp.ai/account`.

**Functional contract:**
- Display confirmation: "Your changes have been saved."
- Instruct: "Return to Pane to continue." (The desktop app re-checks entitlement on focus per ADR-0025.)
- No API calls. No token storage. No subscription verification. Page is informational.

**Error modes:** None within scope — page is static. Stripe handles the redirect; paneapp.ai just renders.

### 2. `/subscribe/success` — Checkout success

**Triggered by:** Customer completes a new Checkout Session (re-subscribing after `canceled` status).

**Uses:** `STRIPE_CHECKOUT_SUCCESS_URL`, defaults to `https://paneapp.ai/subscribe/success`.

**Functional contract:**
- Display confirmation: "Welcome back! Your subscription is active."
- Instruct: "Open Pane to continue." (The app detects the change on next focus.)
- No API calls. Stripe webhook (`invoice.paid`) handles the actual activation in `app_metadata`.

**The page does not distinguish between "first subscribe" and "customer typed the URL"** — and doesn't need to. The reassurance-and-retry content is appropriate either way.

### 3. `/subscribe/cancel` — Checkout abandoned

**Triggered by:** Customer abandons Checkout Session (closes tab, clicks back, times out).

**Uses:** `STRIPE_CHECKOUT_CANCEL_URL`, defaults to `https://paneapp.ai/subscribe/cancel`.

**Functional contract:**
- Display: "No changes were made to your account."
- Reassure: "You can subscribe anytime from within Pane."
- No error framing. Abandonment is a valid customer choice.

## Implementation handoff

This document is the source of truth for whoever builds paneapp.ai's three pages. Visual design and copy are out of scope here — see future paneapp.ai design work for those.

The contract this file pins is functional: when each page renders, what it must convey, what it must not do, how it handles error modes.

The wrapper's env-var configuration of `STRIPE_PORTAL_RETURN_URL`, `STRIPE_CHECKOUT_SUCCESS_URL`, `STRIPE_CHECKOUT_CANCEL_URL` is wrapper-side and tracked in `pane-ecom-backend#8`. Deploy-time coordination (paneapp.ai route changes propagating to wrapper env vars) is a deploy-ops concern, not a spec concern.
