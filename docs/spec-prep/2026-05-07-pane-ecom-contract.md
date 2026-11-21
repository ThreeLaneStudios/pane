# Pane ↔ Ecom API Contract: Sign-in and Single-Active-Machine

**Defined:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Format:** ADR-0023 *spirit*, **sibling-service exception** (see Format Note below)
**Status:** v1 — authoritative for both Pane and ecom-backend
**Tags:** #973, spec-prep
**Locking ADR:** `docs/adr/0027-pane-ecom-api-contract.md`

---

## Format note

ADR-0023 mandates external-system contracts be **captured from source-of-truth**. This document is **not** a capture — it's a **definition**. The ecom backend doesn't yet exist; both Pane (client) and the ecom backend (server) will be authored against this document.

ADR-0023 §"Internal sibling services don't need capture-from-source-of-truth because both sides are authored together" carves out this case from the capture-from-source rule. ADR-0027 documents the exception explicitly and accepts the responsibility that comes with it: Reed's PR review checks both sides' code against this document.

A future auditor: do not expect a `Source: <repo>:<line>`, `Version: <commit>`, `Captured: <date>` header. Expect *this* header. If you're auditing whether Pane's code matches reality, compare Pane's HTTP client implementation against this document, *and* compare the ecom backend's handler implementation against this document. Both sides adapt to this spec.

If the ecom-backend later moves out-of-org (vendor, separate company, etc.), this exception lapses and a real ADR-0023 capture from the new source-of-truth replaces this artifact.

---

## Conventions

- **Base URL:** ecom platform-specific; Pane stores it in app config.
- **Versioning:** all paths prefixed `/v1/`. Breaking changes ship under `/v2/...`. This document's revision date (above) bumps on any non-breaking change.
- **Encoding:** request and response bodies are JSON with `Content-Type: application/json; charset=utf-8`.
- **Authentication:** opaque bearer tokens. Authenticated endpoints expect `Authorization: Bearer <token>`. Anonymous endpoints listed below explicitly.
- **Error envelope:** uniform across all endpoints (see Error envelope section below).
- **Time:** all timestamps are ISO 8601 with timezone (`2026-05-07T12:00:00Z`).
- **Rate limiting:** server returns `429 Too Many Requests` with `Retry-After: <seconds>` header. Pane respects `Retry-After`.

---

## Endpoints

### 1. Email check

Determine whether an email has an account. Drives whether Pane shows the password field (S1b) or the no-account message (S1c).

| | |
|---|---|
| **Path** | `POST /v1/auth/email-check` |
| **Auth** | Anonymous |
| **Rate limit (defensive expectation)** | ~10/min per source IP |

**Request:**

```json
{
  "email": "matt@example.com"
}
```

### Response shape — `POST /v1/auth/email-check`

> **Superseded by ADR-0029 (2026-05-08).** Previous shape included an
> optional `first_name` field on the 200 response, used by the original
> S1b "Welcome back, [first_name]" personalization. ADR-0029 removes
> `first_name` for privacy posture: the email-check endpoint is
> anonymous and rate-limited as a `{exists: bool}` oracle; returning
> `first_name` widened that oracle to a name-disclosure oracle without
> commensurate privacy controls. The previous shape is preserved in
> ADR-0027 §2 body (immutable) for historical reference.

**Response (200):**

```json
{
  "exists": true
}
```

`exists` is the only field on the 200 response. The wrapper performs
the Supabase admin user-lookup, discards everything but the existence
boolean, and returns it. No account-derived metadata (name, created
timestamp, subscription status, etc.) is returned on this endpoint —
those fields are reachable only via authenticated endpoints after a
successful `/v1/auth/signin`.

**Errors:** Standard error envelope per §6. Rate-limit response on this
endpoint follows the same shape; ~10/min per IP per the user-enumeration
mitigation lock.

---

### 2. Sign in

Authenticate, issue a token, return current entitlement snapshot.

| | |
|---|---|
| **Path** | `POST /v1/auth/signin` |
| **Auth** | Anonymous |
| **Rate limit (defensive expectation)** | ~5 attempts per 15min per email |

**Request:**

```json
{
  "email": "matt@example.com",
  "password": "...",
  "install_uuid": "550e8400-e29b-41d4-a716-446655440000",
  "machine_name": "Matt's MacBook Pro"
}
```

| Field | Notes |
|---|---|
| `install_uuid` | Per ADR-0026: install-time UUID stored in Pane local config. Used by ecom as `active_machine_id`. |
| `machine_name` | From `os.hostname()` at sign-in time. Updated server-side on every sign-in. Not refreshed between sign-ins; see ADR-0027 §3 for rename edge case. |

### Response shape — `POST /v1/auth/signin` (entitlement snapshot embedded)

> **Superseded by ADR-0029 (2026-05-08).** Previous shape returned
> `subscription_status` as a 3-value enum (`active`, `lapsed`,
> `cancelled`) and did not include `subscription_action_url`,
> `next_payment_attempt`, `amount_due`, or `currency`. ADR-0029
> widens the enum to 5 values, adds per-customer action URL, and adds
> retry-info primitives. `account_first_name` removed per ADR-0029
> Decision 1. Previous shape preserved in ADR-0027 §2 body (immutable).

**Response (200):**

```json
{
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "token": "k3y9xQz4M2nP8aB7cD1eF5gH9jK2lM4nO6pQ8rS0tU2v",
  "expires_at": "2026-08-05T12:00:00Z",
  "subscription_status": "active",
  "subscription_lapsed_at": null,
  "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
  "active_machine_name": "Matt's MacBook Pro",
  "subscription_action_url": null,
  "next_payment_attempt": null,
  "amount_due": null,
  "currency": null
}
```

| Field | Notes |
|---|---|
| `user_id` | Supabase Auth UUID. Stable identifier for the user, independent of email mutability. |
| `token` | Opaque bearer, 256-bit entropy, base64url-encoded. ~43 characters. |
| `expires_at` | 90 days from issue. Pane stores in keychain alongside token; on expiry, sign back in. |
| `subscription_status` | `active` \| `past_due` \| `unpaid` \| `cancel_at_period_end` \| `canceled` (per ADR-0029). |
| `subscription_lapsed_at` | Timestamp or null. Forward-compat field for future grace-period feature (#15); v1 Pane ignores. |
| `active_machine_id` | UUID string or null. Auto-claim semantics below. |
| `active_machine_name` | String or null. Paired with `active_machine_id`. Used by Pane to display "signed in on [name]" in S8. |
| `subscription_action_url` | URL string or null. Wrapper-decided: Customer Portal for recoverable states, Checkout Session for `canceled`. See ADR-0029 §2.2. |
| `next_payment_attempt` | RFC 3339 UTC timestamp or null. Stripe retry schedule. Populated with `amount_due` + `currency` or all null. |
| `amount_due` | Integer (Stripe cents) or null. Pairs with `currency`. |
| `currency` | ISO 4217 uppercase code or null. Pairs with `amount_due`. |

**Auto-claim semantics on `active_machine_id`:**

| Server state pre-call | Server behavior | Response value |
|---|---|---|
| `active_machine_id` is null | Atomically set to request's `install_uuid` | The new value (= request's `install_uuid`) |
| `active_machine_id` == request's `install_uuid` | No change | Unchanged (= request's `install_uuid`) |
| `active_machine_id` ≠ request's `install_uuid` | No change | The existing value (Pane will see mismatch and show S8) |

**Errors:**

| Status | `error.code` | When | Pane behavior |
|---|---|---|---|
| 400 | `bad_request` | Malformed input | Surface as bug telemetry, generic error UI |
| 401 | `auth_failed` | Wrong password | Inline "Incorrect password" + [Forgot password →] link per §6.1.1 |
| 403 | `email_not_verified` | Email exists but unverified at ecom | "Verify your email — check your inbox" with link to ecom site |
| 403 | `account_disabled` | Manual ecom action (abuse, refund, etc.) | "Account unavailable — contact support" |
| 404 | `account_not_found` | Email doesn't exist (defensive — only reachable if `/email-check` was bypassed) | Same UX as `/email-check` returning false: route to S1c "no account found" state |
| 429 | `rate_limited` | Brute-force protection triggered | "Too many sign-in attempts — try again in [N] seconds" using `Retry-After` |
| 5xx | `internal_error` | Upstream / ecom platform failure | Per X6 case 1: hard-block first-sign-in with retry button |

**Notes:**

- Token is issued even when `subscription_status` is `lapsed` or `cancelled`. Pane gates on the field, not on token presence. The token is needed so customer can call `/v1/account/active-machine` after renewing on the web without re-signing-in.

---

### 3. Entitlement check

Read current subscription state and active_machine_id. Called at launch and on-focus per ADR-0025.

| | |
|---|---|
| **Path** | `GET /v1/account/entitlement` |
| **Auth** | Bearer |
| **Rate limit (defensive expectation)** | Liberal — ≥60/min per token |

**Request:**

```
GET /v1/account/entitlement
Authorization: Bearer k3y9xQz4M2nP8aB7cD1eF5gH9jK2lM4nO6pQ8rS0tU2v
X-Install-UUID: 550e8400-e29b-41d4-a716-446655440000
```

`X-Install-UUID` header: Pane sends current install_uuid for ecom-side observability (helps support diagnose mismatches). Server doesn't gate on it — the response always returns the canonical `active_machine_id`, and Pane compares locally.

### Response shape — `GET /v1/account/entitlement`

> **Superseded by ADR-0029 (2026-05-08).** Previous shape returned
> `subscription_status` as a 3-value enum (`active`, `lapsed`,
> `cancelled`) and did not include `subscription_action_url`,
> `next_payment_attempt`, `amount_due`, or `currency`. ADR-0029
> widens the enum to 5 Stripe-derived values, adds per-customer
> action URL, and adds retry-info primitives. `account_first_name`
> removed per ADR-0029 Decision 1. Previous shape preserved in
> ADR-0027 §3 body (immutable).

**Response (200):**

```json
{
  "subscription_status": "active",
  "subscription_lapsed_at": null,
  "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
  "active_machine_name": "Matt's MacBook Pro",
  "subscription_action_url": null,
  "next_payment_attempt": null,
  "amount_due": null,
  "currency": null
}
```

Same field semantics as sign-in response (no `token` since this is a read-only call against an existing session). See ADR-0029 for full field semantics, status enum mapping, and `subscription_action_url` behavior.

**Example — `past_due` with retry scheduled:**

```json
{
  "subscription_status": "past_due",
  "subscription_lapsed_at": "2026-05-08T00:00:00Z",
  "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
  "active_machine_name": "Matt's MacBook Pro",
  "subscription_action_url": "https://billing.stripe.com/p/session/test_xyz",
  "next_payment_attempt": "2026-05-11T03:00:00Z",
  "amount_due": 800,
  "currency": "USD"
}
```

**Pane behavior on response:**

| Local condition | Action |
|---|---|
| `subscription_status` ≠ `active` | Route to S7 hard-block with status-specific CTA per §6.1.2 (subscription gate takes precedence over machine check) |
| `subscription_status` = `active` AND `subscription_action_url` non-null | Grace-period soft-block toast per §6.1.2.1 |
| `active_machine_id` ≠ local `install_uuid` | Route to S8 take-over UI; display `active_machine_name` |
| Otherwise | Continue to Home / onboarding-resume |

**Errors:**

| Status | `error.code` | When | Pane behavior |
|---|---|---|---|
| 401 | `token_expired` | Token past `expires_at` | Clear keychain, route to S1 sign-in |
| 401 | `token_revoked` | Sign-out elsewhere or manual revocation | Same as `token_expired` |
| 404 | `account_not_found` | Account deleted at ecom after sign-in | Clear keychain, route to S1; per redesign edge cases, local data is orphaned but not auto-cleared |
| 429 | `rate_limited` | Defensive | Respect `Retry-After` |
| 5xx / network | — | Upstream / ecom unreachable | Per ADR-0024/0025: continue with last-known entitlement, no UI degradation |

**Notes:**

- Mismatched `active_machine_id` is **200**, not 403. The customer's account state isn't a resource access denial — it's information Pane uses to route UI. 4xx is reserved for hard errors.

---

### 4. Take over (claim active_machine)

Claim `active_machine_id` for this install. Used by S8 take-over UI when the customer taps [Take over here].

| | |
|---|---|
| **Path** | `POST /v1/account/active-machine` |
| **Auth** | Bearer |
| **Rate limit (defensive expectation)** | ~10/min per account |

**Request:**

```json
{
  "install_uuid": "550e8400-e29b-41d4-a716-446655440000",
  "machine_name": "Matt's MacBook Air"
}
```

**Response (200):**

```json
{
  "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
  "active_machine_name": "Matt's MacBook Air",
  "subscription_status": "active",
  "account_first_name": "Matt"
}
```

**Idempotency:** repeated calls with the same `install_uuid` are no-ops. Last-write-wins on contention between machines (no synchronous race-condition reporting; loser detects on next entitlement check).

**Errors:**

| Status | `error.code` | When | Pane behavior |
|---|---|---|---|
| 401 | `token_expired` / `token_revoked` | | Clear keychain, route to S1 |
| 403 | `subscription_lapsed` | Subscription is `lapsed` | Route to S7 |
| 403 | `subscription_cancelled` | Subscription is `cancelled` | Route to S7 with "cancelled" copy |
| 404 | `account_not_found` | Account deleted at ecom | Clear keychain, route to S1 |
| 429 | `rate_limited` | Defensive | Respect `Retry-After` |
| 5xx | `internal_error` | Upstream | "Couldn't take over — try again" with manual retry; do not auto-retry |

---

### 5. Sign out

Invalidate token + clear active_machine_id server-side.

| | |
|---|---|
| **Path** | `POST /v1/auth/signout` |
| **Auth** | Bearer |
| **Rate limit (defensive expectation)** | Liberal |

**Request:** empty body.

**Response (204):** empty body.

**Side effects on ecom:**

1. Token invalidated (deleted from token table).
2. `active_machine_id` and `active_machine_name` cleared (set to null).

**Errors:**

| Status | `error.code` | When | Pane behavior |
|---|---|---|---|
| 401 | `token_expired` / `token_revoked` | Token already invalid | Clear keychain anyway, route to S1 |
| 5xx | `internal_error` | Upstream | Clear keychain locally regardless; log to telemetry; token expires naturally within 90d |

**Notes:**

- Sign-out is best-effort against ecom but **unconditional locally**. Pane always clears keychain even on server failure — the local trust state is what determines next-launch behavior.

---

## Error envelope

Uniform across all endpoints:

```json
{
  "error": {
    "code": "subscription_lapsed",
    "message": "Subscription is no longer active.",
    "details": {}
  }
}
```

| Field | Notes |
|---|---|
| `code` | Discrete enum string. Pane switches on this; UX copy is keyed off `code`, not `message`. |
| `message` | Human-readable fallback. Server returns English. Pane generally renders its own copy; falls back to `message` only for unknown codes. |
| `details` | Open-ended object. Empty in v1 for all currently-defined codes. Reserved for future per-code structured data. |

### HTTP status conventions

| Status | Meaning |
|---|---|
| 200 | Success with body |
| 204 | Success, no body |
| 400 | Malformed request (Pane bug; do not retry) |
| 401 | Unauthenticated (token bad/expired; route to S1) |
| 403 | Authenticated but not entitled (lapsed, cancelled, disabled, unverified) |
| 404 | Resource not found (typically account deleted) |
| 409 | *Reserved.* Not used in v1. |
| 429 | Rate-limited (respect `Retry-After`) |
| 5xx | Server / upstream error (retryable per below) |

### Retry semantics

Pane's expected retry behavior:

- **5xx + transport-level errors:** exponential backoff with jitter. Base 1s, cap 30s, max 3 attempts. Then surface failure.
- **429:** respect `Retry-After` header verbatim (or 1s if absent). Do not combine with 5xx backoff.
- **4xx:** never retry. These are bugs (400) or user-action-required (401/403/404).

### Defensive expectations on rate limits

Not a contract Pane *negotiates* — expectations Pane should be defensive against:

| Endpoint | Expected limit | Notes |
|---|---|---|
| `/v1/auth/email-check` | ~10/min per IP | Enumeration defense |
| `/v1/auth/signin` | ~5/15min per email | Brute-force defense |
| `/v1/account/entitlement` | ≥60/min per token | Must support legit launch + on-focus traffic |
| `/v1/account/active-machine` | ~10/min per account | |
| `/v1/auth/signout` | Liberal | |

---

## Glossary

| Term | Definition |
|---|---|
| `install_uuid` | UUID generated at Pane install, stored in Pane local config. The `active_machine_id` per ADR-0026. |
| `active_machine_id` | The `install_uuid` ecom currently has on file as the active machine for the account. |
| `active_machine_name` | The `os.hostname()` of the machine when it last signed in or took over. Cosmetic display only. |
| Sign-in | Two-call sequence: `/v1/auth/email-check` then `/v1/auth/signin`. |
| Take-over | Explicit override of an existing `active_machine_id` claim, via `/v1/account/active-machine`. |
| Auto-claim | First-time `active_machine_id` set by `/v1/auth/signin` when no prior claim exists. |

---

## References

- ADR-0027 — this contract's locking ADR
- ADR-0023 — capture discipline (sibling-service exception applies)
- ADR-0013 — opaque-bearer auth precedent
- ADR-0024 — local-only decryption (entitlement-only stance)
- ADR-0025 — call sites: launch + on-focus
- ADR-0026 — install_uuid as active_machine_id
- `docs/spec/onboarding.md` §6.1 — endpoints cited from sign-in flow
- Issue #973 — implementation target for single-active-machine enforcement
