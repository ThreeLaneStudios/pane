# ADR-0028: Ecom Platform Selection — Supabase Auth + Stripe + Thin Wrapper

**Status:** Accepted
**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0027 (Pane ↔ Ecom API Contract)

## Context

ADR-0027 locked the Pane↔ecom wire contract independent of platform choice. This ADR locks which platform sits underneath the thin wrapper that exposes that contract to Pane.

The original ADR-0027 work assumed "build the backend ourselves on Stripe." A follow-up research session evaluated three categories of alternatives:

1. **All-in-one with native machine licensing** — Lemon Squeezy Licensing API, Paddle Mac SDK, Outseta.
2. **Auth-as-a-service + Stripe + thin glue backend** — Clerk + Clerk Billing, Clerk Auth + direct Stripe, Supabase Auth + direct Stripe.
3. **Build-your-own** — Stripe + custom auth + custom DB.

Three findings shaped the decision:

- **License-key platforms don't fit the spec.** Lemon Squeezy and Paddle both have machine-licensing primitives, but their identity model is license-key-based, not email+password. Adopting either invalidates §6.1.1 (sign-in via website credentials). Paddle's licensing primitive additionally ships only as a Mac SDK — no Windows or Linux equivalent — which kills cross-platform Tauri support outright.
- **The wire contract Pane sees doesn't change** as long as we stay on email+password. ADR-0027 survives any platform that supports that identity model, because Pane talks to our wrapper, not the platform directly. This is a meaningful insight: platform selection shifts implementation cost, not Pane-side rework.
- **Multi-tier pricing requires Stripe directly.** Clerk Billing (the integrated Clerk subscription primitive) handles single-tier cleanly but is awkward for multi-tier flows: tier upgrade/downgrade with proration math, mid-cycle plan changes, one-time add-ons, metered components. Stripe's primitives for these are mature and date-pinned. Cost is identical (Stripe takes its standard cut either way); only glue differs.

The cross-section landed on Supabase Auth + direct Stripe + thin wrapper.

### What if we didn't decide

- **Build-your-own ships ~1 week later.** Auth code (password hashing, session management, password reset, email verification) and Stripe webhook plumbing (idempotency, retries, signature verification) take ~1 week of Stia's time. With launch 8 days out (per the project timeline), that week is the difference between launching with #973 done and launching with it deferred.
- **Clerk path adds a third-party seam Pane doesn't yet exercise.** Pane already integrates with Supabase (sync via Supabase CRDT, encrypted notifications via Supabase REST poller, cross-cutting auth handling per `src-tauri/src/notifications.rs:372` and `src-tauri/src/sync_db.rs`). Reed's audit at `docs/audits/2026-05-07-supabase-auth-spec-vs-reality.md` documents the existing surface. Choosing Clerk for ecom would mean operating two third-party identity-adjacent platforms in production for what amounts to one launch's worth of value.
- **Lemon Squeezy / Paddle break the spec.** Adopting either forces §6.1 rewrite from email+password to license-key entry. Pre-decided constraint forbids this.

## Decision

**Use Supabase Auth + direct Stripe + thin wrapper.** Three load-bearing decisions:

### 1. Identity provider: Supabase Auth

Supabase Auth handles email+password sign-up, sign-in, email verification, and password reset. The Pane wrapper calls Supabase Auth's admin API (service role) for user lookups and `app_metadata` writes; calls Supabase Auth's user API for sign-in credential validation. Pane never talks to Supabase Auth directly.

Why Supabase Auth specifically (vs Clerk):

- **Already in Pane.** Reduces production third-party surface area.
- **Free tier covers 50K MAU**, well above launch and growth targets (100 week-one, 10K post-launch).
- **`app_metadata` vs `user_metadata` distinction** is the right primitive for ecom-controlled fields. Service-role-only writes mean Pane (with only the user's own session) cannot mutate `subscription_status`, `active_machine_id`, etc. Privilege boundary is in the platform, not in our code.
- **JWT stability** — Supabase has a documented backward-compatibility commitment per their auth repo (semver-stable; breaking changes only on major versions with deprecation notice).
- **Open-source path exists.** If Supabase becomes problematic, self-hosting Supabase Auth (gotrue) is documented and supported. Identity portability is the strongest of any candidate platform.

### 2. Subscription billing: direct Stripe

The wrapper integrates with Stripe directly:

- Stripe Checkout for sign-up flow (on the ecom website, out of scope for this ADR).
- Stripe Customer Portal for self-service billing management.
- Stripe webhooks (`customer.subscription.created/updated/deleted`, `invoice.payment_failed`) drive `app_metadata.subscription_status` denormalization in Supabase.
- Wrapper's `/v1/account/entitlement` reads `app_metadata.subscription_status` — no Stripe API call on the hot path.

Why direct Stripe (vs Clerk Billing or Outseta):

- **Multi-tier maturity.** Stripe handles tier proration, plan switching, add-ons, trials, and coupons natively. Clerk Billing is newer; Outseta has more friction. Same Stripe cut either way.
- **Deprecation discipline.** Stripe's API uses date-pinned versions (e.g., `2026-04-22.dahlia`); breaking changes ship under new dates with multi-year overlap. Maps cleanly to ADR-0023's versioning discipline.
- **Standard primitive.** Subscription management with Stripe is a well-trodden integration path with mature tooling (Stripe CLI, test mode, webhooks playground).

### 3. Wrapper: thin server, ecom-backend repo, owns supabase/migrations/

The wrapper is a small server (language/framework deferred to implementation). Five Pane-facing endpoints per ADR-0027. Internally:

- **Holds the Supabase service role key** (env var, secret manager in production).
- **Holds Stripe secret key** (same).
- **Owns its own database state**, persisted in Supabase Postgres on the same project, in its own schema (e.g., `ecom`). Tables: `wrapper_tokens` (token table per ADR-0027 §1), any reconciliation state for Stripe webhook idempotency.
- **Schema location: `ecom-backend repo owns `supabase/migrations/`.** This ADR names the location explicitly to remove ambiguity. Pane's repo does not author or apply ecom-related migrations. The Supabase project may be the same one Pane uses (single Supabase project, multiple schemas) or a separate project — implementation-time decision; either preserves the schema-ownership boundary.
- **Validates incoming opaque tokens** by lookup in `wrapper_tokens`. Mints new ones on sign-in.

The wrapper repo is separate from Pane's repo. Same org. Reed coverage extended to it as a precondition for #973 implementation (sub-task: file Reed-coverage extension PR for ecom-backend repo).

## Consequences

### Positive

- **Backend cost: ~5–7 days** (vs ~1–2 weeks build-your-own). Auth absorbed by Supabase, Stripe webhook plumbing is well-trodden.
- **No Pane-side rework.** ADR-0027's wire contract survives. Stia's #973 work continues against the unchanged spec-prep file.
- **Reuses existing operational familiarity.** Supabase is already in Pane's stack; ops/monitoring patterns transfer.
- **Free at our entire growth runway** (Supabase free tier 50K MAU; Stripe charges only on transactions, no fixed fee).
- **Schema ownership boundary is clean.** ecom-backend repo owns `supabase/migrations/`. Pane repo doesn't touch ecom schema.

### Negative

- **Two third-party captures to maintain.** Supabase Auth + Stripe. Per ADR-0023 discipline — re-verify pre-launch and quarterly thereafter, or on observed breaking changes.
- **Wrapper holds two high-privilege credentials.** Supabase service role + Stripe secret key. Compromise of either is significant. Mitigated by standard secret management practices; ADR-0027 §Risks names the specific failure modes for the service role key.
- **Stripe webhook reliability is on us.** Missed webhook → drifted `subscription_status`. Mitigation via idempotency keys + reconciliation job; sub-task to file.

### Risks

- **Supabase Auth and Stripe both move.** ADR-0023's stale-capture risk applies. Reed's PR review for the ecom-backend repo includes capture-version checks.
- **`app_metadata` accidental misuse** — discussed in ADR-0027 §Risks. Lint or test rule sub-task before launch.
- **Single Supabase project for Pane data + ecom data** (if we choose that path) creates a blast radius coupling. If the Supabase project is unavailable, both sync (Pane data) and ecom (entitlement gating) degrade simultaneously. ADR-0024/0025 already accommodate ecom unreachable; sync has its own offline tolerance per existing spec. Bounded but worth naming. Mitigation alternative: separate Supabase projects for sync and ecom — more operational overhead, less coupling.

## Alternatives Considered

**Lemon Squeezy Licensing API.** All-in-one merchant-of-record with native machine licensing. **Rejected** because the identity model is license-key, not email+password. Adopting LS forces §6.1.1 rewrite. Additionally: the [well-documented LS issue around customer reinstall losing instance_id](https://lemonsqueezy.nolt.io/515) is exactly our X8 case 2 — adopting LS would inherit that bug. MoR (tax handling) is real value but evaluable post-launch.

**Paddle Billing API + Mac SDK.** Similar shape to Lemon Squeezy. **Rejected** for the identity model AND because the licensing primitive is Mac-only — no Windows/Linux SDK exists, kills cross-platform Tauri.

**Outseta.** All-in-one membership platform with email+password auth. **Rejected** because it's web-CRM-focused (live chat, help desk, email marketing) — most of the platform's value is for surfaces Pane doesn't have. Pricing ($37–47/mo + 2% transaction fee on entry plan) is higher than Supabase + Stripe at our scale, and the active_machine_id layer would need to be built on top of Outseta's custom-fields API just like on Supabase or Clerk.

**Clerk + Clerk Billing.** Modern auth-as-a-service with native subscription primitive. **Rejected** because Clerk Billing handles multi-tier flows awkwardly (proration, mid-cycle changes, add-ons), and Matt is open to multi-tier pricing in v1. Same Stripe cut either way; Stripe directly handles those flows natively. Clerk Auth alone (without Clerk Billing) was considered but introduces a new third-party platform when Supabase already does the job.

**Clerk Auth + direct Stripe.** Considered as the v2 of the Clerk evaluation after the multi-tier finding. **Rejected** because Pane already uses Supabase; adopting Clerk here introduces a new third-party identity platform for ecom while sync/notifications continue using Supabase. Operational and capture-discipline overhead. Functionally near-equivalent to Supabase Auth + Stripe; tiebreaker goes to the platform already in production.

**Build-your-own (Stripe + custom auth + custom DB).** Maximum control, no third-party identity dependency. **Rejected** for ~1 week of additional Stia time at 8 days to launch. Brand-promise concern (customer credentials at a third party) is bounded — ADR-0024 says ecom is entitlement gating only, local data stays local-only-decryptable. Future option: if multi-tenant Tier 1 hosted introduces patterns Supabase doesn't fit, migrating off Supabase is supported (open-source Supabase / gotrue self-hosted is a documented path). Identity portability is acceptable.

**Defer the platform decision; build the wrapper interface and decide later.** Same shape as ADR-0023's spike-and-exit. **Rejected** because the wrapper's implementation language and deployment story depend on the platform — deferring just moves the decision into implementation, where it costs more to make. The contract survives platform choice; the decision should land before code.

## Sub-tasks named by this ADR

The following work items follow from this ADR landing. Filed as preconditions for #973 implementation kicking off:

1. **File ecom-backend repo creation issue** — repo location, initial scaffold, `supabase/migrations/` directory.
2. **File Reed-coverage extension issue** — extend Reed's PR-review scope to the new repo, including ADR-0023 capture-vs-code checks.
3. **File webhook reconciliation job issue** — periodic job to re-read Stripe subscription state and reconcile `app_metadata.subscription_status` drift.
4. **File `app_metadata` discipline issue** — lint/test rule that fails when ecom-relevant code paths write to `user_metadata` instead of `app_metadata`.
5. **File capture re-verification cadence** — pre-launch verification of Supabase Auth + Stripe captures; quarterly recurrence post-launch.

These are scoping items, not implementation. #973 itself is unchanged.

## References

- Contract ADR: ADR-0027 (Pane ↔ Ecom API Contract)
- Capture discipline: ADR-0023 (External Contract Capture Discipline)
- Auth precedent: ADR-0013 (Auth and Multi-Tenancy Seam) — opaque bearer
- Pre-decided constraints:
  - ADR-0024 (Local-only Decryption)
  - ADR-0025 (Ecom Check Cadence)
  - ADR-0026 (Install UUID as active_machine_id)
- Reed audit (Supabase already in Pane): `docs/audits/2026-05-07-supabase-auth-spec-vs-reality.md`
- Spec-prep captures:
  - `docs/spec-prep/2026-05-07-pane-ecom-contract.md` (Pane↔wrapper, sibling-service)
  - `docs/spec-prep/2026-05-07-supabase-auth-platform-capture.md` (third-party, ADR-0023)
  - `docs/spec-prep/2026-05-07-stripe-platform-capture.md` (third-party, ADR-0023)
- Implementation issue: #973
- Future work:
  - MoR / tax handling (Stripe Tax or migrating to Lemon Squeezy if international scale demands)
  - Tier 1 hosted refresh-token design (post-launch)
  - Single vs split Supabase projects (Pane data + ecom data) — operational decision pre-launch
