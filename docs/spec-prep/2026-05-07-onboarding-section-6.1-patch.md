# §6.1 Patch — Endpoint Citations for Pane ↔ Ecom Contract

**Source:** Spec session 2026-05-07
**Target file:** `docs/spec/onboarding.md`
**Related:** ADR-0027, `docs/spec-prep/2026-05-07-pane-ecom-contract.md`

## What this patch does

Adds endpoint citations to `docs/spec/onboarding.md` §6.1 (Sign-in) so the prose narrative references the locked contract for every Pane → ecom call. Citations link to the spec-prep document for the wire-shape detail; the prose stays focused on UX flow.

The patch is by-subsection text directives rather than a unified diff because §6.1's exact landed prose was outside this session's input set. Apply each subsection's content to the matching §6.1.x; integrate with existing prose at the natural insertion points (typically end of subsection, after the UX narrative).

---

## §6.1.1 — Sign-in (S1 / S1b / S1c)

**Append to subsection:**

```markdown
**Endpoints.** S1's existence check uses `POST /v1/auth/email-check`; S1b's password submit uses `POST /v1/auth/signin`. Both are anonymous. Wire shapes, request/response fields, and error codes are documented in `docs/spec-prep/2026-05-07-pane-ecom-contract.md` §1–§2 per ADR-0027.

**Token storage.** The sign-in response carries `token`, `expires_at`, `subscription_status`, `active_machine_id`, `active_machine_name`, and `account_first_name`. Pane stores `token` and `expires_at` in OS keychain (per ADR-0024). On expiry, Pane re-runs S1 sign-in. Refresh-flow primitives are out of scope for v1 and deferred to Tier 1 hosted (per ADR-0027 §1).

**S1c "no account found" state** is driven by `/v1/auth/email-check` returning `{exists: false}`. Pane never reaches `/v1/auth/signin` for a non-existent email under normal flow; the 404 `account_not_found` error response on `/signin` is defensive only.

**Ecom unreachable on first sign-in.** Per X6 case 1, Pane hard-blocks with retry. The `/v1/auth/signin` call surfaces 5xx or transport-level errors as "Couldn't reach Pane. Check your connection. [Retry]". No degraded mode for first-sign-in (no cached entitlement available yet).

**`machine_name` field.** Pane sends `os.hostname()` on sign-in; ecom stores it as `active_machine_name`. There is no separate update endpoint — renames between sign-ins propagate only on next sign-in. Cosmetic-only staleness; see ADR-0027 §3 for the rename edge case.
```

---

## §6.1.2 — Subscription state at sign-in

**Replace or augment** existing text describing how Pane evaluates subscription state at sign-in:

```markdown
After successful sign-in, Pane evaluates the response's `subscription_status` field:

| Value | Pane action |
|---|---|
| `active` | Continue to single-active-machine check (§6.1.3) |
| `lapsed` | Route to S7 hard-block (renew screen) |
| `cancelled` | Route to S7 with cancellation copy |

**The token is issued regardless of subscription state.** Pane stores it in keychain even on lapsed/cancelled responses. The token is needed so the customer can call `/v1/account/active-machine` after renewing on the web, without re-signing-in.

**`subscription_lapsed_at`** is captured forward-compat for #15 (grace-period feature); v1 ignores. v1 lapse → S7 immediately, no countdown.

**Subscription state on subsequent launches** is read via `/v1/account/entitlement` (§6.1.3), not via re-sign-in. Sign-in re-evaluation only happens when the cached token expires or is invalidated.
```

---

## §6.1.3 — Single-active-machine check

**Replace or augment** existing text describing the active-machine check:

```markdown
Pane runs the single-active-machine check at two call sites per ADR-0025:

1. **At launch**, after sign-in completes (or after a cached token is loaded from keychain on a returning launch).
2. **On-focus**, when the Pane window gains focus after being backgrounded.

Both call sites use `GET /v1/account/entitlement` (per `docs/spec-prep/2026-05-07-pane-ecom-contract.md` §3, ADR-0027). The response's `active_machine_id` is compared against the local `install_uuid` (per ADR-0026):

| Comparison | Pane action |
|---|---|
| `subscription_status` ≠ `active` | Route to S7 (subscription gate takes precedence over machine check) |
| `active_machine_id` matches local `install_uuid` | Continue (no UI change) |
| `active_machine_id` does not match | Show S8 take-over UI, displaying `active_machine_name` |

**Auto-claim on sign-in.** When `/v1/auth/signin` receives a sign-in for an account whose `active_machine_id` is null at ecom, ecom atomically sets it to the request's `install_uuid` and returns the new value. Pane sees `active_machine_id` matching its local `install_uuid` and continues without S8. This is the first-machine-ever case; no separate claim call is needed.

**Take-over (S8 confirmation).** When the customer taps [Take over here], Pane calls `POST /v1/account/active-machine` with `install_uuid` and `machine_name`. The endpoint claims unconditionally (last-write-wins on contention).

| Response | Pane behavior |
|---|---|
| 200 | Continue to onboarding-resume / Home |
| 403 `subscription_lapsed` / `subscription_cancelled` | Route to S7 |
| Other 4xx/5xx | Surface "Couldn't take over — try again", manual retry |

The displaced machine notices on its next entitlement check (next launch or next on-focus) and silently signs out per the redesign edge case spec.

**Ecom unreachable on entitlement check.** Per ADR-0024/0025, Pane continues with last-known entitlement. No UI degradation. Local data access is independent of ecom reachability (ADR-0024 brand promise).

**Sign-out (Settings, future surface).** `POST /v1/auth/signout` invalidates the token server-side AND clears `active_machine_id` server-side (per ADR-0027 §5). Pane clears keychain locally unconditionally — even on 5xx — so the next launch starts at S1 regardless of server reachability at sign-out time.

**Machine name handling.** `machine_name` is sourced from `os.hostname()` at sign-in and take-over time. It updates only on those events — there is no separate update endpoint. With 90-day token lifetime, a renamed machine's name as displayed on *other* machines' S8 prompts can be stale up to one sign-in cycle. Cosmetic only; the `active_machine_id` (install_uuid) is correct throughout. See ADR-0027 §3.
```

---

## Cross-reference updates elsewhere in §6.1

- Anywhere §6.1 currently references "ecom backend" or "ecom check" without specifying which call, update to specify the endpoint per the new contract.
- The ADR list at the top or bottom of §6.1 (or the file's header references section, depending on `onboarding.md` convention) should add **ADR-0027** alongside ADRs 0024–0026.
- If §6.1 cross-references the X6 case 1 hard-block decision, add a parenthetical noting the `/v1/auth/signin` 5xx mapping defined in the contract.

## Application checklist

- [ ] §6.1.1 endpoint paragraph appended (anonymous endpoints called out)
- [ ] §6.1.1 token storage paragraph appended (keychain + 90d + no refresh)
- [ ] §6.1.1 `machine_name` paragraph appended (rename edge case noted)
- [ ] §6.1.2 subscription_status decision table added/updated
- [ ] §6.1.2 token-issued-on-lapsed clarification added
- [ ] §6.1.3 launch + on-focus call sites cited to `/v1/account/entitlement`
- [ ] §6.1.3 auto-claim semantics described
- [ ] §6.1.3 take-over endpoint + 403-on-lapsed behavior cited
- [ ] §6.1.3 sign-out behavior described (clears active_machine_id)
- [ ] ADR-0027 added to §6.1's ADR cross-reference list
- [ ] All "ecom backend" / "ecom check" prose updated with specific endpoint paths
