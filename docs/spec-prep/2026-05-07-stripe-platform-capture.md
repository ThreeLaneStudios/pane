# Stripe API — Platform Contract Capture

**Capture Date:** 2026-05-07
**Capture Format:** ADR-0023 (third-party, real source-of-truth capture)
**Author:** Matt Carano, Claude (spec session)
**Tags:** #973, spec-prep, ecom-platform
**Locking ADR:** `docs/adr/0028-ecom-platform-selection.md`
**Used By:** ecom-backend wrapper (per ADR-0027)

---

## Format note

This is a **real ADR-0023 capture** (not the sibling-service exception). The wrapper code authored against this capture is the Pane organization's; Stripe is the third-party. Reed's PR review on the wrapper repo verifies wrapper code against these shapes; stale captures are flagged for re-verification per ADR-0023's versioning policy.

Distinct from `docs/spec-prep/2026-05-07-pane-ecom-contract.md` (which is define-not-capture, sibling-service exception, governs Pane↔wrapper). Both apply.

## Source of truth

- **Subscription object:** https://docs.stripe.com/api/subscriptions/object (accessed 2026-05-07)
- **Subscription lifecycle:** https://docs.stripe.com/billing/subscriptions/overview (accessed 2026-05-07)
- **Subscription webhooks:** https://docs.stripe.com/billing/subscriptions/webhooks (accessed 2026-05-07)
- **Event types:** https://docs.stripe.com/api/events/types (accessed 2026-05-07)
- **API versioning:** Stripe API versions are date-pinned (e.g., `2026-04-22.dahlia`). This capture pins to **`2026-04-22.dahlia`** as the API version the wrapper will be built against.

## Versioning note

**Stability:** unusually high. Stripe pins API versions by date; new versions are released as new dates with multi-year deprecation overlap. Subscription object shape changes are infrequent and clearly announced. The webhook event vocabulary (`customer.subscription.created/updated/deleted`, `invoice.payment_failed`) is among the most stable parts of the API.

**Re-verification cadence per ADR-0023:** pre-launch (mandatory) + quarterly thereafter, OR on observed Stripe changelog announcements affecting the captured shapes.

**Trust level:** high. Official docs are accurate against API behavior; date-pinning makes drift detection mechanical (compare wrapper's pinned version against current); Stripe's deprecation discipline is exemplary in the industry.

---

## Endpoints used by the wrapper

The wrapper interacts with Stripe in three modes: **read** (fetch subscription state during reconciliation), **webhook receive** (denormalize state changes into Supabase `app_metadata`), and **portal generate** (link customers to self-service billing). Pane never communicates with Stripe directly.

### 1. Subscription object shape

The full object as returned by Stripe API and webhook payloads:

```json
{
  "id": "sub_1MowQVLkdIwHu7ixeRlqHVzs",
  "object": "subscription",
  "customer": "cus_Na6dX7aXxi11N4",
  "status": "active",
  "current_period_start": 1679609767,
  "current_period_end": 1682288167,
  "cancel_at_period_end": false,
  "canceled_at": null,
  "cancel_at": null,
  "ended_at": null,
  "trial_start": null,
  "trial_end": null,
  "items": {
    "object": "list",
    "data": [
      {
        "id": "si_...",
        "price": {
          "id": "price_...",
          "product": "prod_...",
          "unit_amount": 999,
          "currency": "usd",
          "recurring": { "interval": "month" }
        },
        "quantity": 1
      }
    ]
  },
  "metadata": {},
  "created": 1679609767,
  "currency": "usd",
  "default_payment_method": "pm_...",
  "collection_method": "charge_automatically",
  "billing_cycle_anchor": 1679609767
}
```

**Wrapper extracts:**

| Stripe field | Used for |
|---|---|
| `id` | Subscription identifier (logging, reconciliation) |
| `customer` | Stripe Customer ID — link to Supabase user via lookup table or `app_metadata.stripe_customer_id` |
| `status` | Mapped to `app_metadata.subscription_status` per §3 below |
| `current_period_end` | Used for `subscription_lapsed_at` calculation if status flips lapsed |
| `cancel_at_period_end` | Forward-compat for grace-period feature (#15) |
| `items.data[0].price.id` | Tier identifier (forward-compat for multi-tier) |

Other fields are read on reconciliation but not currently denormalized.

---

### 2. Subscription status enum

Stripe's full status enum (per `subscription.status`):

| Stripe status | Meaning |
|---|---|
| `incomplete` | Initial payment failed; recoverable within 23 hours |
| `incomplete_expired` | Initial payment failed; not recoverable, terminal |
| `trialing` | Active trial, no payment yet |
| `active` | Subscription active, current invoices paid |
| `past_due` | Payment failed for current period; retries in progress |
| `canceled` | Customer or merchant canceled; terminal |
| `unpaid` | Payment failed and retries exhausted; subscription kept but not collecting |
| `paused` | Trial ended without payment method; no invoices generated |

---

### 3. Status mapping (Stripe → ADR-0027 enum)

The wrapper denormalizes Stripe's 8-value status enum into ADR-0024's 3-value enum (`active` / `lapsed` / `cancelled`):

| Stripe status | Pane enum | Reasoning |
|---|---|---|
| `active` | `active` | Direct match |
| `trialing` | `active` | Customer has access during trial |
| `past_due` | `lapsed` | Payment retry in progress; recoverable with action |
| `unpaid` | `lapsed` | Payment retries exhausted; recoverable if customer updates payment method |
| `incomplete` | `lapsed` | Initial payment failed; recoverable for 23 hours |
| `paused` | `lapsed` | Recoverable if customer adds payment method |
| `canceled` | `cancelled` | Terminal |
| `incomplete_expired` | `cancelled` | Terminal — initial payment failed and recovery window closed |

**Implementation note:** mapping table lives in wrapper code as a single function. Reed PR review verifies the table matches this capture; drift in mapping is a Reed-blocking finding.

---

### 4. Webhook events the wrapper subscribes to

Wrapper registers a Stripe webhook endpoint that processes the following events. All others are ignored at v1.

| Event type | Wrapper action |
|---|---|
| `customer.subscription.created` | Write `app_metadata.subscription_status` (mapped per §3); set `app_metadata.account_first_name` from Customer's `name` field |
| `customer.subscription.updated` | Re-map status, update `app_metadata.subscription_status`; update `subscription_lapsed_at` if transitioning to lapsed |
| `customer.subscription.deleted` | Set `app_metadata.subscription_status = "cancelled"` |
| `invoice.payment_failed` | No direct status write — Stripe will follow up with `subscription.updated` to `past_due`. Logged for observability. |
| `customer.updated` | If `name` changed, update `app_metadata.account_first_name` |

**Webhook payload shape:**

```json
{
  "id": "evt_1234",
  "object": "event",
  "api_version": "2026-04-22.dahlia",
  "created": 1715091840,
  "type": "customer.subscription.updated",
  "data": {
    "object": { /* the Subscription object per §1 */ }
  },
  "request": { "id": "req_...", "idempotency_key": null },
  "livemode": true
}
```

**Webhook signature verification:** Stripe sends a `Stripe-Signature` header. Wrapper MUST verify the signature using the webhook signing secret before processing. **Unsigned or signature-mismatch payloads are rejected with 401.** This is non-negotiable security hygiene; failure to verify means anyone can post events to the webhook endpoint.

```
Stripe-Signature: t=1715091840,v1=<HMAC>,v0=<...>
```

Wrapper uses Stripe's official SDK helper (`stripe.webhooks.constructEvent` in JS, or equivalent in other languages) — never roll signature verification by hand.

---

### 5. Idempotency

Stripe webhooks may be delivered more than once. Wrapper MUST be idempotent per `event.id`:

1. Receive webhook → verify signature → extract `event.id`.
2. Check `event.id` against wrapper's processed-events table (in `ecom` schema).
3. If already processed → return 200 immediately, do not re-process.
4. If new → process, insert `event.id` into processed-events table.

**Stripe's retry policy:** if wrapper returns non-2xx, Stripe retries 3 times over a few hours. After that, manual intervention required (Stripe Dashboard → Webhook → Resend). Reconciliation job (per ADR-0028 sub-task) catches missed events.

---

### 6. Reconciliation job (defensive against missed webhooks)

Per ADR-0028's risk mitigation: a periodic job reads each user's subscription status from Stripe directly and updates `app_metadata` if drift detected.

**Path:** `GET /v1/subscriptions/{subscription_id}` (read-only, idempotent)

**Headers:**
```
Authorization: Bearer <secret key>
Stripe-Version: 2026-04-22.dahlia
```

**Response:** the same Subscription object per §1.

**Job semantics:**
- Cadence: hourly initially; tunable post-launch.
- For each user with `app_metadata.stripe_subscription_id`, fetch current state, compare to `app_metadata.subscription_status` after mapping per §3, write if drift.
- Drift events logged for observability (high drift = webhook reliability issue).

---

### 6a. Signin-time Stripe API fallback (#1144)

Per ADR-0031 §Race Window: when a customer signs in during the window between webhook events 7a and 7b, the wrapper queries Stripe directly as a belt-and-suspenders fallback.

**Path:** `GET /v1/subscriptions?customer={stripe_customer_id}&status=all&limit=1`

**Headers:**
```
Authorization: Bearer <secret key>
Stripe-Version: 2026-04-22.dahlia
```

**Trigger:** `stripe_customer_id` present in `app_metadata` AND `subscription_status` absent/null.

**Response:** Subscription list (at most 1 entry). Mapped through §3 status mapping. Result written back to `app_metadata.subscription_status` (write-through closes race permanently).

**Error handling:** Stripe API failure → log error, fall through to `canceled` coercion. Customer hits S7 degraded-but-functional; webhook lands on subsequent request.

**Frequency:** At most once per user (write-through prevents repeat calls). Only fires during the race window between webhook delivery events.

---

### 7. Customer Portal session (used by ecom website, not by wrapper-Pane path)

Captured for completeness; not on the wrapper-to-Pane critical path. The ecom website calls `POST /v1/billing_portal/sessions` to mint short-lived URLs that customers click to manage their subscription. This is out of scope for #973 but in scope for the broader ecom-backend repo.

---

## Auth model used by the wrapper

| Key | Usage | Reach |
|---|---|---|
| **secret key** (`sk_live_...` or `sk_test_...`) | All wrapper-side Stripe API calls | Full Stripe account access. Wrapper holds; never logged; never sent to client. |
| **publishable key** (`pk_live_...` or `pk_test_...`) | Stripe.js on ecom website | Public-safe; used at sign-up checkout. Out of scope for wrapper. |
| **webhook signing secret** (`whsec_...`) | HMAC verification of webhook payloads | Wrapper holds; never sent anywhere. Per-endpoint (different secrets for different webhook endpoints). |

**Secret key handling per ADR-0028:**
- Stored in environment variable / secret manager only.
- Never logged.
- Never sent to client.
- Scope: only operations listed in this capture.

---

## Behavior the wrapper relies on

### API version pinning

Wrapper sends `Stripe-Version: 2026-04-22.dahlia` on every request. Pinning ensures response shapes match this capture even after Stripe ships newer API versions. Upgrading the pinned version is a deliberate ADR-0023 re-capture event, not a silent upgrade.

### Subscription status transitions

Wrapper relies on Stripe's documented status transitions. If Stripe adds a new status (e.g., a hypothetical `payment_action_required`), the mapping function in §3 has no case for it — wrapper code should default to a safe state (recommendation: treat unknown statuses as `lapsed` until mapping is updated, log loudly). Capture re-verification catches this.

### Webhook event delivery is at-least-once

Wrapper MUST be idempotent. Captured in §5; failure to be idempotent results in duplicate `app_metadata` writes (mostly harmless but logged) or, worse, duplicate side effects in any downstream code (alerts, emails).

### Stripe's MoR vs non-MoR boundary

Stripe is **not** a Merchant of Record by default. Tax handling is the merchant's responsibility. Stripe Tax is available as an add-on. Per ADR-0028, MoR is deferred — wrapper uses Stripe directly without Tax for v1; revisit post-launch if international scale demands.

---

## Forbidden patterns (Reed PR review checklist)

For ecom-backend wrapper PRs, Reed's review verifies:

1. **Webhook signature verification on every webhook handler.** No exceptions, no "this endpoint is internal" hand-waves.
2. **Idempotency on webhook processing.** Per-event-id deduplication via processed-events table.
3. **No secret key in client-bound code paths.** Secret key usage is server-side only.
4. **API version pinned in every request.** No requests without explicit `Stripe-Version` header.
5. **Status mapping function matches §3 exactly.** Drift is a Reed-blocking finding.
6. **Captured shape match.** JSON deserialization of Stripe responses must match the shapes in this document.

---

## Glossary

| Term | Definition |
|---|---|
| **secret key** | High-privilege Stripe API key. Wrapper holds; never exposed. |
| **publishable key** | Public Stripe API key. Used by ecom website, not wrapper. |
| **webhook signing secret** | HMAC secret for verifying Stripe-sent webhook payloads. |
| **Stripe-Version** | Date-pinned API version header. Wrapper pins `2026-04-22.dahlia`. |
| **MoR** | Merchant of Record. Stripe is not MoR by default; tax/VAT is merchant's responsibility. |

## References

- ADR-0027 (contract this capture implements)
- ADR-0028 (platform selection)
- ADR-0023 (capture discipline)
- Source URLs cited in "Source of truth" section above
- Implementation issue: #973
