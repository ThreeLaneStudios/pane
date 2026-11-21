# ADR-0030: paneapp.ai Stripe Return-URL Contract

**Status:** Accepted  
**Date:** 2026-05-09  
**Relates to:** ADR-0027 (wire contract), ADR-0029 (entitlement response shape), ecom-backend #8 (Stripe URL env vars)

## Context

The ecom wrapper generates Stripe Customer Portal and Checkout Session URLs via `subscription_action_url` (ADR-0029 §2.2). Stripe redirects the customer back to a URL after the Portal/Checkout flow completes. These return URLs must:

1. Exist as pages on paneapp.ai
2. Be configured as env vars in the wrapper (not hardcoded)
3. Have defined functional contracts (what each page conveys, what it must not do)

## Decision

### Three env vars, one shared

The wrapper uses three env vars for Stripe return URLs:

| Env var | Default (production) | Used by |
|---|---|---|
| `STRIPE_PORTAL_RETURN_URL` | `https://paneapp.ai/account` | Both Portal callsites (S7 hard-block + grace-period soft-block) |
| `STRIPE_CHECKOUT_SUCCESS_URL` | `https://paneapp.ai/subscribe/success` | Checkout Session (re-subscribe after `canceled`) |
| `STRIPE_CHECKOUT_CANCEL_URL` | `https://paneapp.ai/subscribe/cancel` | Checkout Session (abandoned re-subscribe) |

**Both Portal callsites share `STRIPE_PORTAL_RETURN_URL`.** The S7 hard-block and grace-period soft-block both generate Portal sessions; the return page is the same in both cases because the customer's post-Portal state is identical (they just updated their payment method or reactivated).

### Per-page functional contracts

#### `/account` (Portal return)
- **Renders when:** Customer returns from Stripe Customer Portal (payment method update, plan change, or cancellation)
- **Must convey:** "Changes saved. Return to Pane to continue." + link/instruction to reopen the desktop app
- **Must not:** Make API calls, store tokens, or attempt to verify subscription state. The app handles re-verification on next focus (ADR-0025 cadence).

#### `/subscribe/success` (Checkout success)
- **Renders when:** Customer completes a new Checkout Session (re-subscribe after `canceled` status)
- **Must convey:** "Subscription active. Open Pane to continue." + reassurance that the app will detect the change
- **Must not:** Attempt to activate the subscription (Stripe webhook handles this). Page is informational only.

#### `/subscribe/cancel` (Checkout abandoned)
- **Renders when:** Customer abandons Checkout Session (closes tab, clicks back, etc.)
- **Must convey:** "No changes made. You can try again from Pane anytime."
- **Must not:** Treat abandonment as an error. The customer may have intentionally decided not to re-subscribe.

### Error modes

All three pages are brochure-style — they don't call backend APIs. The only error mode is "page doesn't load" (DNS, hosting down), which is outside this contract. No Stripe session ID parsing, no subscription verification, no webhook waiting.

### Startup validation

The wrapper fails fast if any of the three env vars are missing (implemented in `src/index.js` startup checks per ecom-backend PR #14).

## Consequences

- paneapp.ai must serve three routes matching the env var defaults
- Wrapper deploy requires all three env vars set
- Both Portal callsites (S7 and grace-period) use the same return URL — no per-callsite routing needed
- Return pages are static/brochure — no backend integration complexity
