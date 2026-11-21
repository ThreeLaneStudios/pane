# ADR-0027: Pane ↔ Ecom API Contract for Sign-in and Single-Active-Machine Enforcement

**Status:** Retired — Superseded by ADR-0070 (2026-11-09); previously superseded in part by ADR-0029 (§2, §3)
**Superseded by:** ADR-0070. The ecom wrapper (`api.paneapp.ai`) is shut down and has no Pane-side consumer: sign-in targets Supabase GoTrue directly, and the entitlement, email-check, active-machine and sign-out endpoints this contract defined are no longer called.
**Date:** 2026-05-07 (revised same day for platform binding)
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplemented by:** ADR-0028 (Ecom Platform Selection — Supabase Auth + Stripe + thin wrapper)

## Revision note (2026-05-07)

This ADR was authored to lock the wire contract between Pane and the ecom backend independent of the platform underneath. The same-day platform-research session (see ADR-0028) selected Supabase Auth + direct Stripe + thin wrapper. **The wire contract Pane codes against did not change.** This revision updates only the relationship notes and implementation references; all five endpoints, request/response shapes, and error semantics are identical to the original lock. Stia's #973 work continues against the unchanged spec-prep file.

## Context

Single-active-machine enforcement (#973) is the implementation target for ADR-0024 (local-only decryption — ecom is entitlement-gating only), ADR-0025 (ecom check at launch + on-focus), and ADR-0026 (install_uuid as active_machine_id). Those ADRs lock *what Pane needs from ecom*; this ADR locks *the wire shape*.

The contract documents the Pane-facing API our thin wrapper exposes. Per ADR-0028 the wrapper is implemented as a small server that proxies to Supabase Auth (identity + token issuance) and Stripe (subscription billing), denormalizes ecom-controlled state into Supabase `app_metadata` (service-role-only), and exposes the five endpoints below to Pane.

### Relationship to ADR-0023 (revised)

ADR-0023 mandates that external-system contracts be captured from source-of-truth at spec time. Two layers of contract apply here, and the discipline distinguishes them:

- **Pane ↔ wrapper** — both authored together, sibling-service exception applies, this document is the canonical reference, no source-of-truth capture required. Format adapted accordingly (define-not-capture).
- **wrapper ↔ Supabase Auth** and **wrapper ↔ Stripe** — third-party services with versioned APIs, **real ADR-0023 captures required**. Filed as `docs/spec-prep/2026-05-07-supabase-auth-platform-capture.md` and `docs/spec-prep/2026-05-07-stripe-platform-capture.md`. Reed's PR review checks wrapper code against those captured shapes.

A future auditor reading this ADR should expect the Pane-facing capture (`docs/spec-prep/2026-05-07-pane-ecom-contract.md`) to look like a definition rather than a source-of-truth capture, *and* should expect the two third-party captures to look like real ADR-0023 captures with source-path / version / capture-date headers. Both apply; neither replaces the other.

### What if we didn't decide

(unchanged from original lock)

- §22C/§826 recurrence shape on the Pane↔wrapper seam if the contract is left implicit.
- Take-over UX has no contract floor — S8 might display partial fields.
- Sign-out invalidation becomes a quiet failure if the contract doesn't pin what gets cleared.
- Tier 1 hosted transition becomes a contract rewrite without a documented v1.

Additional failure mode now visible (post-platform pivot): with Supabase + Stripe under the wrapper, contract-implicit Pane code would be coupled directly to Supabase JWT shape and Stripe subscription enum — exactly the §826 risk pattern, on a third-party seam that ADR-0023 was designed to mechanize discipline against. The wrapper indirection insulates Pane; this ADR's contract is what makes the indirection meaningful.

## Decision

**Pane and the wrapper communicate via the contract defined at `docs/spec-prep/2026-05-07-pane-ecom-contract.md`.** The wrapper proxies to Supabase Auth + Stripe per ADR-0028. Five load-bearing decisions:

### 1. Auth: opaque bearer, 90-day, no refresh

Wrapper-issued opaque bearer tokens, 256 bits of entropy, base64url-encoded. Issued by the wrapper on sign-in (after validating credentials against Supabase Auth), server-tracked in a wrapper-owned token table, invalidated server-side on sign-out. Stored client-side in OS keychain per ADR-0024.

**Why wrapper-issued and not pass-through Supabase JWT:** Supabase JWTs default to 1-hour expiry and are paired with refresh tokens. ADR-0027's lock is "opaque bearer, 90-day, no refresh" per ADR-0013 precedent. Pass-through would require either (a) extending Supabase JWT expiry to 90 days, which fights Supabase's defaults and introduces a non-revocable window, or (b) building refresh-token handling into Pane. Wrapper-issued tokens preserve the lock cleanly: Pane sees opaque bearers; the wrapper's internal Supabase session is implementation detail, refreshed server-side as needed.

The wrapper's token table maps `wrapper_token → supabase_user_id + expires_at`. Sign-out deletes the row. ADR-0028 specifies the table lives in the wrapper's own database (Supabase Postgres in the same project, separate schema, accessed via service role).

### 2. Sign-in: separate email-check endpoint

Two anonymous endpoints — `/v1/auth/email-check` and `/v1/auth/signin`. Per the original lock, with the same per-IP rate-limiting expectation (~10/min) on `/email-check` to bound the user-enumeration oracle. Wrapper implements `/email-check` via Supabase admin API user-lookup (service role), returns `{exists: bool}`. Wrapper implements `/signin` by calling Supabase `auth.signInWithPassword`, then minting its own opaque token and returning the entitlement snapshot.

### 3. Single-active-machine: install_uuid + machine_name in Supabase app_metadata

`active_machine_id` = `install_uuid` per ADR-0026. Stored at `user.app_metadata.active_machine_id`. Paired with `user.app_metadata.active_machine_name` populated from `os.hostname()` at sign-in.

**Why `app_metadata` and not `user_metadata`:** Supabase Auth distinguishes:

- `user_metadata` — writable by the user themselves via their own JWT (any authenticated user can mutate their own).
- `app_metadata` — writable **only via service role** (admin operations).

`active_machine_id`, `active_machine_name`, `subscription_status`, and `account_first_name` are ecom-controlled state, not user-controlled. They MUST live in `app_metadata`. Pane never writes these directly; only the wrapper, holding the service role key, writes via Supabase admin API. Pane reads them only through wrapper endpoints. **Pane never communicates with Supabase directly for these fields.**

Rename edge case is unchanged from the original lock: cosmetic-only staleness up to 90 days; mitigation path forward-compatible.

Auto-claim semantics on sign-in are unchanged: if `app_metadata.active_machine_id` is null at sign-in time, the wrapper atomically sets it to the request's `install_uuid` (admin API write) and returns it.

### 4. Take-over while lapsed: 403, fail fast

Unchanged from original lock. `POST /v1/account/active-machine` returns 403 with `subscription_lapsed` (or `subscription_cancelled`) when the wrapper's view of subscription state — denormalized from Stripe via webhook into `app_metadata.subscription_status` — is not active.

### 5. Sign-out clears active_machine_id

Unchanged from original lock. `POST /v1/auth/signout` invalidates the wrapper-issued token (deletes from wrapper token table) AND clears `app_metadata.active_machine_id` and `app_metadata.active_machine_name` (admin API write). Wrapper does NOT call Supabase's own signout — Supabase session lifecycle is internal to the wrapper; only the Pane-facing token is what matters to Pane.

### 6. Error envelope and HTTP conventions

Unchanged from original lock. Uniform error envelope `{error: {code, message, details}}`, HTTP status conventions, retry semantics all per the original.

### 7. URL versioning

Unchanged. `/v1/...`. Breaking changes ship `/v2/...`.

## Consequences

### Positive

(plus, beyond the original lock)

- **Backend build cost reduced from ~1–2 weeks to ~5–7 days** per ADR-0028's analysis. Auth + Stripe webhook plumbing absorbed by Supabase + Stripe respectively; wrapper becomes a thin shim around five endpoints.
- **Reed's audit at `docs/audits/2026-05-07-supabase-auth-spec-vs-reality.md`** documents that Supabase is already in Pane (sync, notifications). Adopting Supabase Auth for ecom builds on existing operational familiarity rather than introducing a new platform dependency.
- **Schema location is unambiguous.** Per ADR-0028, the ecom-backend repo owns `supabase/migrations/` for the wrapper's own tables (token table, any denormalized state). Pane's repo does not author or apply ecom-related Supabase migrations.

### Negative

- **Two real ADR-0023 captures to maintain.** Supabase Auth and Stripe both have versioned APIs that drift over time. Captures need re-verification on a defined cadence (proposed: pre-launch, then quarterly thereafter, per ADR-0023's stable / breaking-on-major / unstable framework). Stripe's API is unusually stable (date-pinned, deprecation cycles measured in years); Supabase Auth is semver-stable but actively developed. Both are tracked in the captures.
- **Wrapper holds service role key.** This is a high-privilege credential. Wrapper deployment must protect it (env var, secret manager). If compromised, attacker can read/write arbitrary `app_metadata` for any user. Mitigated by: scoped service role usage in wrapper code (only operations needed), audit logging of admin-API calls, principle of least privilege in wrapper endpoint handlers.
- **Token table is operational state.** Wrapper's `wrapper_token → supabase_user_id` mapping is a small but real piece of state we own. Backup/restore included in ADR-0028's deployment scope.

### Risks

- **Supabase outage cascades to ecom unreachable.** ADR-0024/0025 already accommodate ecom unreachable (cached entitlement, no UI degradation), so this is bounded — but worth monitoring; Supabase status page should be on the on-call dashboard.
- **Stripe webhook failures cause `subscription_status` drift.** If a webhook is missed (network, deploy, signature failure), the denormalized status in `app_metadata` can lag reality. Mitigation: webhook idempotency keys, Stripe's retry mechanism (3 retries over hours), reconciliation job that periodically re-reads subscription state from Stripe API and updates `app_metadata` if drift detected.
- **`app_metadata` vs `user_metadata` mistake.** If a wrapper endpoint accidentally writes to `user_metadata` instead of `app_metadata`, the field becomes user-mutable from any signed-in client (security vulnerability — customer could write their own `subscription_status: active`). Mitigation: wrapper code helpers that ONLY take `app_metadata` paths; lint rule or test that fails on `user_metadata` usage in ecom-related code paths; Reed PR review explicitly checks this on every wrapper PR.

## Alternatives Considered

(unchanged decisions retained from original lock; new platform-related alternatives moved to ADR-0028)

The original ADR's alternatives — JWT vs opaque, unified vs separate signin endpoint, short-token+refresh, sync race-reporting on take-over, machine_name update endpoint, defer-capture-until-built — all stand. ADR-0028 documents the platform alternatives (Lemon Squeezy, Paddle, Outseta, Clerk, Supabase, build-your-own).

## References

- Spec section: `docs/spec/onboarding.md` §6.1 (Sign-in flow)
- Spec-prep capture (Pane↔wrapper, sibling-service exception): `docs/spec-prep/2026-05-07-pane-ecom-contract.md`
- ADR-0023 capture (wrapper↔Supabase Auth, third-party): `docs/spec-prep/2026-05-07-supabase-auth-platform-capture.md`
- ADR-0023 capture (wrapper↔Stripe, third-party): `docs/spec-prep/2026-05-07-stripe-platform-capture.md`
- Reed audit (Supabase already in Pane): `docs/audits/2026-05-07-supabase-auth-spec-vs-reality.md`
- Related ADRs:
  - ADR-0028 (Ecom Platform Selection)
  - ADR-0013 (Auth and Multi-Tenancy Seam) — opaque bearer precedent
  - ADR-0023 (External Contract Capture Discipline)
  - ADR-0024 (Local-only Decryption)
  - ADR-0025 (Ecom Check Cadence)
  - ADR-0026 (Install UUID as active_machine_id)
- Implementation issue: #973
