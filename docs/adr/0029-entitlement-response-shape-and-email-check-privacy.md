# ADR-0029: Entitlement Response Shape and Email-Check Privacy Posture

**Status:** Accepted — Superseded in part by ADR-0070 (2026-11-09)
**Superseded by:** ADR-0070 for Decision 2 and everything downstream of it (the 5-value `subscription_status` enum, `subscription_action_url`, the retry-info fields, S7 CTA mapping) and for Decision 1's wire shape (`POST /v1/auth/email-check` no longer exists). Decision 1's privacy posture — no account-derived metadata on an anonymous endpoint — stands as a principle for any future account surface.
**Date:** 2026-05-08
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** ADR-0027 §2 (email-check response shape) and §3 (entitlement response output shape) — see "Scope of supersession" below
**Supplements:** ADR-0028 (Ecom Platform — Supabase + Stripe + thin wrapper)

> **Implementation note for Matt:** This ADR carries the delta from ADR-0027 per playbook §2 (ADRs are immutable; supersede, don't edit). ADR-0027's status banner gets updated to "Superseded in part by ADR-0029" but its body stays unchanged — the historical wire shape lives there. The new wire shape is in this ADR's Decision section and lands in `docs/spec-prep/2026-05-07-pane-ecom-contract.md` with explicit supersession markers per the companion deliverables.
>
> Two earlier ADRs (0019 superseded 0006; 0020 followed) set the precedent for partial-supersession ADRs in this repo. This is the third use of the pattern.

## Context

ADR-0027 locked the wire contract between Pane and the wrapper. Two design decisions surfaced after that lock require contract-level support that the original §2 and §3 shapes don't carry. Plus two UI states surfaced during the same design review that flow from the new shape rather than requiring additional contract decisions.

### What the design decisions are

**(a) S1b drops "Welcome back, Matthew" personalization in favor of "Welcome back."** This is a privacy-posture call: the email-check endpoint is anonymous and rate-limited at ~10/min per IP per ADR-0027 §2, but it still carries an enumeration oracle (returning `{exists: bool}`). Returning `{exists: true, first_name: "Matthew"}` for a known account expands that oracle from "is this email registered" to "is this email registered AND its registered name." The personalization payoff is small; the privacy cost is non-trivial. The original ADR-0027 §2 had `first_name` as an optional field on the email-check response shape; that field is removed.

**(b) S7 collapses to one screen with three CTA variants based on Stripe-derived subscription status.** The original ADR-0027 returned `subscription_status` as a coarse-grained enum (`active`, `lapsed`, `cancelled`). Stripe's actual lifecycle has finer-grained states with materially different recovery paths:

- `past_due` / `unpaid` — payment failed, customer needs to update payment method. Recovery via **Stripe Customer Portal** session.
- `cancel_at_period_end` — customer requested cancellation but still has access through the period end. Recovery via **Customer Portal** to undo the cancellation.
- `canceled` — terminal state; subscription cannot be reactivated. Recovery via **new Stripe Checkout Session** entirely.

A single resubscribe URL cannot serve all three states. Stripe treats `canceled` as terminal: the existing subscription record is gone, and the only way to bring the customer back is a fresh Checkout. The Customer Portal cannot resurrect a canceled subscription. Pane therefore needs (i) finer-grained status to drive CTA copy, and (ii) a per-customer action URL the wrapper picks server-side based on which Stripe operation applies.

### Two UI states surfaced concurrently

These states use the new contract primitives but don't require additional contract decisions. Specifying them here so the supersession is complete:

- **Grace-period soft-block.** Subscription technically `active` but payment retry pending — the customer is still functional but the wrapper has a payment-method action they should take. UI: in-app toast with the action CTA, not a hard-block screen. Distinct from S7. Signal: `subscription_status = active` AND `subscription_action_url` is non-null.
- **S1b rate-limited.** Distinct from wrong-password. The wrapper returns 429 on `/v1/auth/signin` after N failed attempts; UI shows countdown + disabled Sign In button + no [Forgot password] link. Signal: 429 response with `details.retry_after_seconds` per the existing error envelope (ADR-0027 §6).

### What's not changing

The five-endpoint shape is preserved. Auth model (opaque bearer, 90-day, no refresh — ADR-0027 §1) is preserved. `install_uuid` + `machine_name` storage in `app_metadata` (ADR-0027 §3 storage decisions) is preserved. Take-over-while-lapsed semantics (§4) and sign-out clearing (§5) are preserved. Error envelope (§6) and URL versioning (§7) are preserved. **This ADR changes only the response payload shapes for email-check and entitlement; it does not touch the endpoint surface, auth model, or storage decisions.**

### Scope of supersession

Specifically:

- ADR-0027 §2's email-check response shape — `first_name` field removed.
- ADR-0027 §3's entitlement response output — `subscription_status` enum widened from 3 values (`active`, `lapsed`, `cancelled`) to 5 (`active`, `past_due`, `unpaid`, `cancel_at_period_end`, `canceled`); `subscription_action_url` field added; `next_payment_attempt`, `amount_due`, `currency` fields added (Stripe-native retry-info primitives for grace-period banner copy). Sign-in response (§2's signin shape) carries the same expanded fields, since the original lock had sign-in returning the entitlement snapshot inline.

Everything else in ADR-0027 stays. The ADR-0027 file gets a status-banner update; its body is not edited.

## Decision

### 1. Email-check returns `{exists: bool}` only

```
POST /v1/auth/email-check
Request:  { "email": "string" }
Response (200): { "exists": true | false }
```

No `first_name` field. The wrapper must not return any account-derived metadata on this endpoint. Implementation: the wrapper performs the Supabase admin user-lookup, discards everything but the existence boolean, and returns it.

This pulls the email-check response down to the minimum information that supports the S1 / S1c branching decision. Anything more is a privacy regression.

### 2. Entitlement and sign-in responses carry expanded status enum + per-customer action URL

The `/v1/account/entitlement` and `/v1/auth/signin` responses both return the entitlement snapshot. The shape:

```
Response (200):
{
  "active_machine_id":       "string | null",  // unchanged
  "active_machine_name":     "string | null",  // unchanged
  "subscription_status":     "active | past_due | unpaid | cancel_at_period_end | canceled",
  "subscription_action_url": "string | null",  // new
  "next_payment_attempt":    "string | null",  // new — RFC 3339 / ISO 8601 UTC timestamp
  "amount_due":              "integer | null", // new — Stripe-native smallest currency unit
  "currency":                "string | null",  // new — ISO 4217 uppercase code, paired with amount_due
  // ...sign-in response also carries token, expires_at; entitlement response does not
}
```

`account_first_name` is removed from the sign-in response too — it was only ever consumed by S1b personalization, which is dropped per Decision 1's privacy posture. (S1b sees `email` from local session storage if it needs to display anything customer-specific.)

The three retry-info fields (`next_payment_attempt`, `amount_due`, `currency`) carry Stripe-native primitives so Pane can render localized payment-retry copy ("$8.00 on May 11" in en-US, "8,00 € am 11. Mai" in de-DE) without pushing locale formatting into the wrapper. Details in §2.4.

#### 2.1 Status enum values and meanings

| Value | Stripe correspondence | Pane UI |
|---|---|---|
| `active` | Subscription is current and paid | Continue (entitled). Grace-period soft-block toast if `subscription_action_url` is non-null. |
| `past_due` | Payment failed, Stripe is retrying | S7 hard-block, "Update Payment" CTA |
| `unpaid` | Payment retries exhausted, subscription suspended | S7 hard-block, "Update Payment" CTA |
| `cancel_at_period_end` | Customer requested cancel; access until period end | S7 hard-block, "Keep Subscription" CTA |
| `canceled` | Subscription terminated, no access | S7 hard-block, "Resubscribe" CTA |

These are the wrapper's denormalized view of Stripe subscription state — see "Consequences" for the architectural coupling this introduces.

#### 2.2 `subscription_action_url` semantics

- **Non-null whenever the customer should take an action.** The wrapper computes the URL server-side based on the customer's current Stripe state.
- **For `past_due`, `unpaid`, `cancel_at_period_end`:** Customer Portal session URL (Stripe billing portal scoped to this customer's subscription).
- **For `canceled`:** new Checkout Session URL (fresh subscription purchase).
- **For `active` with no pending payment issue:** null.
- **For `active` with payment retry pending (grace-period):** non-null Customer Portal URL — Pane surfaces an in-app toast rather than S7.

Pane stays dumb on URL routing. The wrapper picks Portal vs Checkout based on Stripe state; Pane just opens the URL in the customer's browser when they click the CTA.

#### 2.3 Mapping to S7 CTA copy

| Status | CTA label |
|---|---|
| `past_due` / `unpaid` | "Update Payment" |
| `cancel_at_period_end` | "Keep Subscription" |
| `canceled` | "Resubscribe" |

The CTA copy is Pane's choice — the wrapper just provides the URL. S7's screen logic switches on status to pick copy; URL is pulled from the same response.

#### 2.4 Retry-info fields: `next_payment_attempt`, `amount_due`, `currency`

The grace-period soft-block banner needs to display "$8.00 on May 11" — concrete, customer-actionable copy. Three fields on the entitlement response carry the Stripe-native primitives Pane uses to render that copy.

**`next_payment_attempt`** — string (RFC 3339 / ISO 8601 UTC timestamp) or null. When non-null, indicates the time at which Stripe will retry the failing invoice. When null, no retry is currently scheduled.

**`amount_due`** — integer (Stripe-native smallest currency unit — cents for USD, pence for GBP, etc.) or null. The amount Stripe will attempt to charge on the next retry.

**`currency`** — string (ISO 4217 uppercase code: "USD", "GBP", "EUR", etc.) or null. Pairs with `amount_due`.

##### Invariant: paired population

The three fields are populated together or all null. Either:

- All three non-null (a retry is scheduled), or
- All three null (no retry is scheduled).

The wrapper enforces this invariant. Pane consumers can rely on it: a non-null `amount_due` always has a non-null `currency` and `next_payment_attempt`. No partial-population states are valid.

##### When the fields are populated

Populated whenever Stripe has a scheduled payment retry on the customer's subscription invoice. In Stripe's lifecycle this means:

- **Grace-period state** (this ADR's `subscription_status = active` AND `subscription_action_url` non-null) — Stripe is mid-smart-retry on a failed invoice while the subscription is still `active`. Fields populated.
- **`past_due`** — typically populated while Stripe continues dunning retries.
- **`unpaid`** — may be populated if dunning continues after the unpaid transition. Often null once dunning exhausts.

Null in all other states:

- `active` with no payment issue — no retry scheduled.
- `cancel_at_period_end` — customer requested cancel; no retry needed.
- `canceled` — terminal; no future payments scheduled.
- `past_due` / `unpaid` after Stripe has stopped retrying (terminal dunning failure).

This is a slight expansion beyond a literal "past_due / unpaid only" reading — grace-period (`active` with retry pending) needs the same fields populated for the banner to be functional. The wrapper keys field population off "is a retry scheduled," not off the status enum directly.

##### Locale boundary: wrapper Stripe-native; Pane formats client-side

The wrapper returns Stripe-native shape — cents, ISO 4217 currency code, UTC ISO 8601 timestamp. Pane does the locale-aware rendering using the customer's OS locale:

- **Currency formatting** via `Intl.NumberFormat(locale, { style: "currency", currency: <code> })` — turns `(800, "USD")` into `"$8.00"` for en-US, `"€8,00"` for de-DE, etc.
- **Date formatting** via `Intl.DateTimeFormat(locale, { month: "short", day: "numeric" })` or equivalent — turns `"2026-05-11T03:00:00Z"` into `"May 11"` for en-US, `"11. Mai"` for de-DE, etc.
- **Composition** — Pane controls the copy template (`"${amount} on ${date}"`) so the surrounding language (`"on"` vs `"am"` vs ja-JP positional) localizes too.

The wrapper does not format. Pushing locale formatting into the wrapper would (a) require the wrapper to know each customer's display locale, (b) couple wrapper-side i18n to Pane's UI copy needs, and (c) lose the "wrapper as Stripe denormalizer, Pane as UX renderer" separation. v1 keeps the wrapper Stripe-native and Pane locale-aware.

##### Nullable, not omit

The three fields are always present in the JSON. Value is `null` when not applicable. Three reasons:

- **Consistency.** `subscription_action_url` is already nullable; matching pattern keeps the response shape uniform.
- **Stripe-native consistency.** Stripe's own API uses `null` for absent values rather than omitting fields.
- **Consumer ergonomics.** Rust's `Option<T>` and TypeScript's `T | null` typecheck cleanly with always-present-but-nullable shape; optional-key access patterns add validator noise without payload-size benefit.

See "Alternatives Considered" for the rejected omit-when-null alternative.

### 3. Grace-period soft-block uses existing primitives

Triggered by `subscription_status = active` AND `subscription_action_url` non-null. UX is a non-blocking in-app toast with the URL as the CTA target; Pane behavior in all other respects is the same as the normal `active` path (chat enabled, no overlay).

Banner copy renders the retry context for the customer using the §2.4 fields: `"<formatted amount> on <formatted date>"` — e.g., `"$8.00 on May 11"` in en-US, formatted Pane-side per §2.4's locale boundary. The CTA points at `subscription_action_url`.

This is a derivation from the contract decision in §2, not an additional contract field. The wrapper sets `subscription_action_url` to a Customer Portal URL whenever Stripe has a pending payment retry, even while subscription_status is still `active`.

> **Design call worth flagging.** Matt's directive said "grace-period flows from the entitlement response shape, not new contract decisions." The cleanest fit is the `subscription_action_url`-non-null-while-active interpretation above. Alternative interpretations (e.g., a separate `payment_retry_pending` boolean) would require expanded contract scope, which the directive rules out. If wrapper implementation surfaces a different signal during build, file a follow-up amendment.

### 4. S1b rate-limited uses existing error envelope

429 response on `/v1/auth/signin` with the existing error envelope (ADR-0027 §6 — `{error: {code, message, details}}`):

```
Response (429):
{
  "error": {
    "code": "rate_limited",
    "message": "Too many sign-in attempts.",
    "details": { "retry_after_seconds": <integer> }
  }
}
```

`retry_after_seconds` is the countdown UI consumes. Existing error envelope, no contract change.

## Rationale

### Privacy posture on email-check (Decision 1)

The personalization payoff is small (a name in a welcome message), the privacy cost is non-trivial (anonymous endpoint becomes a name-disclosure oracle), and the current rate-limiting (~10/min per IP) is calibrated for "is this email registered" not "give me a name for this email." Dropping `first_name` from the response shape closes the disclosure side of the oracle without changing the per-IP rate-limiting story.

S1b loses the warmth of "Welcome back, Matthew" → "Welcome back." Acceptable trade. Personalization can come back later via post-sign-in surfaces (Settings, Home greeting) where the customer is already authenticated and the name is not anonymous-endpoint-derivable.

### Stripe enum coupling (Decision 2)

This is the architectural decision worth naming explicitly. **Pane's subscription_status enum values are now Stripe-derived.** The wrapper denormalizes Stripe's subscription lifecycle states (`active`, `past_due`, `unpaid`, `incomplete`, `incomplete_expired`, `trialing`, `paused`, `canceled`) down to the five Pane cares about. Pane's UI logic switches on enum values that match Stripe's vocabulary.

This is a coupling the wrapper does not insulate Pane from. ADR-0027's wrapper indirection insulates Pane from Stripe's *API* — Pane never calls Stripe directly, only the wrapper. But the *semantics* of subscription state are Stripe's; Pane reads them through the wrapper but cannot avoid them. If Stripe deprecates `cancel_at_period_end` or adds a new state (e.g., `paused`), Pane's enum has to track or the wrapper has to map.

Three options were considered:

1. **Keep ADR-0027's collapsed enum (`active`, `lapsed`, `cancelled`).** Rejected because it loses the recovery-path distinction. Pane can't tell "Update Payment" from "Resubscribe" without finer status, and the single resubscribe URL approach can't serve `canceled` (Stripe-side terminal) and `past_due` (Stripe-side recoverable) with one URL.

2. **Have Pane construct URLs locally based on collapsed enum.** Rejected because the URLs are Stripe-issued, not Pane-constructed (Customer Portal sessions and Checkout Sessions both require server-side creation with secret keys). Pane has no path to construct them; only the wrapper does.

3. **Wrapper-issued status + wrapper-issued URL (this decision).** Pane stays dumb on routing logic; the wrapper does the Stripe-coupled work; the Stripe coupling lives in the spec where it can be audited.

Option 3 is the cleanest cut. The wrapper insulates Pane from Stripe's API but not Stripe's vocabulary; the vocabulary is the contract. Future Tier 1 hosted transitions inherit this coupling — that's known, documented, acceptable.

### Per-customer action URL (Decision 2.2)

URL routing is server-decided to keep Pane stateless and Stripe-blind. If Pane decided "past_due → Portal, canceled → Checkout" client-side, the routing logic would have to know Stripe semantics, which is exactly what the wrapper is supposed to insulate. Wrapper-decided URL is the right cut: wrapper knows the customer's full Stripe state and which API call (Portal session create vs Checkout session create) is appropriate; it returns a URL Pane just opens.

### Reusing existing error envelope (Decision 4)

The error envelope ADR-0027 §6 already specifies `{error: {code, message, details}}` with `details` as a free-form map. Rate-limit responses fit without expansion: `details.retry_after_seconds` is a use of existing primitives, not a new contract field. Spec'ing the convention here so the wrapper implementation is unambiguous.

## Consequences

### Positive

- **S7 actually maps to recovery paths.** The original three-value enum collapsed `past_due` (recoverable via Portal) with `cancelled` (recoverable only via fresh Checkout) — a customer staring at S7 with one CTA had a 33% chance of being routed to the wrong Stripe surface. The new mapping fixes this.

- **Pane stays dumb on URL routing.** Per the directive, Pane has no `if past_due then portal else checkout` logic. URL is opaque from Pane's perspective; just open it.

- **Email-check oracle tightens.** `first_name` disclosure is closed without weakening the existence check.

- **Grace-period and rate-limit UX have spec homes.** Both flow from primitives in §2 and §6 of ADR-0027 plus this ADR's expansion; neither requires additional contract decisions.

- **Wrapper insulates Pane from Stripe's API.** Pane never calls Stripe; never holds Stripe secret keys; never constructs Stripe URLs. The wrapper does all of that.

### Negative

- **Pane is now coupled to Stripe-derived status semantics.** This is real and worth documenting honestly. The five enum values are Stripe's vocabulary, not Pane's. If Stripe deprecates `cancel_at_period_end` (as their `cancellation_details` schema deprecation in 2024 suggested might come), Pane has to either track or let the wrapper map to a frozen Pane-side vocabulary. The wrapper-side mapping option exists but is currently not in scope; the v1 cut is "Pane reads Stripe's vocabulary directly through the wrapper." Tier 1 hosted may revisit.

- **Future Stripe lifecycle states get added to the wrapper, not Pane.** When the wrapper sees a Stripe state Pane's enum doesn't recognize (e.g., a future `incomplete_expired` or `paused`), the wrapper must map it to one of Pane's five values. The mapping policy is a wrapper-implementation decision but should be documented; absent that, the wrapper could silently fall through to `canceled` or `unpaid` and surface unexpected S7 copy. **Filed as a follow-up: "Wrapper Stripe-state mapping policy" — wrapper-internal spec, not Pane-side.**

- **One more ADR file in the index.** Same as ADR-0019's note: not free, mitigated by the supersession banner forcing readers to ADR-0029.

- **S1b loses personalization.** "Welcome back, Matthew" → "Welcome back." Cosmetic regression; design-team-accepted per the directive.

### Risks

- **Wrapper mapping bugs surface as wrong S7 CTA.** A wrapper that mis-maps Stripe's `unpaid` to `canceled` would put the customer on a Resubscribe path when Update Payment would have worked. Detection: integration tests against Stripe's test-mode billing fixtures, asserting wrapper status mapping. Mitigation: §11 audit cadence on the Stripe → wrapper denormalization path, not Pane-side.

- **Grace-period soft-block fires too aggressively.** If the wrapper sets `subscription_action_url` non-null on every transient Stripe state change (e.g., during payment-method updates), customers see toast spam. Mitigation: wrapper-side debounce policy. Filed as a follow-up: "Grace-period toast cadence" — wrapper implementation concern.

- **Stripe enum drift.** Stripe could add a state Pane's enum doesn't anticipate. Mitigation: wrapper-side mapping policy (above); §11 audit catches drift; the wrapper is the chokepoint for handling this gracefully.

- **`subscription_action_url` is a long-lived Stripe URL.** Customer Portal session URLs typically have short TTL (~5 minutes per Stripe's defaults). If Pane caches the entitlement response and the customer clicks the CTA an hour later, the URL may be expired. Mitigation: Pane refreshes entitlement on every CTA click intent (a fast cheap call), not on every render. Filed as an implementation note for #973-followup.

## Alternatives Considered

**Keep `first_name` on email-check with stricter rate-limiting on the disclosure path.** Considered: serve `first_name` only when the request comes from an IP that already saw a `{exists: true}` response in the last N minutes. Rejected as security theater — the rate-limiting is for the existence oracle, not the disclosure oracle, and once the existence oracle is open the disclosure oracle is just a small additional turn of the same crank.

**Keep ADR-0027's collapsed status enum and let Pane choose Portal vs Checkout based on a separate `recovery_type` field.** Rejected because the routing logic is Stripe-coupled either way, just at a different field name. Cleaner to put the coupling on the status enum where the lifecycle vocabulary is canonical.

**Have the wrapper return a Pane-side vocabulary (e.g., `needs_payment`, `expiring_soon`, `expired`) that the wrapper maps from Stripe states internally.** Rejected for v1 because the indirection adds a wrapper-side mapping layer for marginal benefit — the spec coupling is the same; the only difference is which side names the enum. Reconsidered for Tier 1 if the Stripe coupling becomes a friction point.

**Make `subscription_action_url` always non-null (a default URL even on `active`-no-action).** Rejected because it would prevent the grace-period derivation in Decision 3 — `null` while `active` is the signal "no action needed." Conflating "no action" with "default action" loses information.

**Omit retry-info fields (`next_payment_attempt`, `amount_due`, `currency`) when not applicable, instead of returning `null`.** Considered briefly. Rejected because (a) `subscription_action_url` is already nullable on the same response and consistency wins; (b) Stripe's own API uses `null` for absent values rather than omitting fields, so wrapper-side denormalization is cleaner with `null`; (c) Rust's `Option<T>` and TypeScript's `T | null` typecheck cleanly with always-present-but-nullable shape, while optional-key access patterns add validator noise without payload-size benefit. Payload size difference is negligible.

**Format payment amount and date inside the wrapper (return `"$8.00 on May 11"` as a string).** Considered. Rejected because the wrapper doesn't know the customer's display locale (and shouldn't — locale is OS-side context that Pane has direct access to but the wrapper doesn't). Pushing formatting into the wrapper would require either (a) the wrapper accepting a `locale` parameter on every entitlement call and maintaining locale tables, or (b) locking display copy to en-US and accepting i18n regression. Both options are worse than the Stripe-native primitives + Pane-side `Intl.NumberFormat` / `Intl.DateTimeFormat` boundary in §2.4.

**Lock the rate-limit details inside ADR-0029 contract surface (a dedicated 429 response shape).** Rejected because the existing error envelope already supports rate-limit signaling via `details`; adding a 429-specific shape would break uniformity. Spec'ing the `details.retry_after_seconds` convention is sufficient.

## References

- Superseded (in part): ADR-0027 §2 (email-check response shape) and §3 (entitlement response output shape). ADR-0027's status banner updates to "Superseded in part by ADR-0029" as part of this filing; ADR-0027 body unchanged per immutability rule.
- Companion deliverables (this session):
  - `docs/spec-prep/2026-05-07-pane-ecom-contract.md` — updated in place with explicit supersession markers at §2 and §3 (see `spec-prep-update-pane-ecom-contract-for-adr-0029.md` in this session)
  - `docs/spec/onboarding.md` §6.1.1 and §6.1.2 — updated for the new shape, rate-limited state, grace-period soft-block (see `spec-onboarding-6-1-update-for-adr-0029.md`)
- Supplemented: ADR-0028 (Ecom Platform — Supabase + Stripe + thin wrapper). The Stripe enum coupling this ADR documents is a consequence of ADR-0028's platform pick; the coupling lives in this ADR's spec text where it can be audited.
- Supersession precedent: ADR-0019 (superseded ADR-0006), ADR-0020 (followed pattern). Dev Hygiene Playbook §2 (ADR immutability rule). The bar for this supersession: substantive design decisions (privacy posture, Stripe lifecycle mapping) requiring contract-level changes that ADR-0027's original framing doesn't carry — meets the supersession bar; this is not a clarifying-supersession.
- Stripe lifecycle reference (background, not a Pane spec dependency): Stripe subscription states documented at stripe.com/docs/billing/subscriptions/overview; Customer Portal at stripe.com/docs/customer-management; Checkout Sessions at stripe.com/docs/payments/checkout. Wrapper is the chokepoint; Pane never calls these directly.
- Follow-ups filed:
  - Wrapper Stripe-state mapping policy (wrapper-internal spec, not Pane-side)
  - Grace-period toast cadence (wrapper implementation concern)
  - `subscription_action_url` TTL handling on stale entitlement (Pane implementation note for #973-followup)
