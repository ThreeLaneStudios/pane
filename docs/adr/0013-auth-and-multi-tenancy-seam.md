# ADR-0013: Auth and Multi-Tenancy Seam for the Pane Gateway

**Status:** Accepted
**Date:** 2026-04-27
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The Pane Gateway (ADR-0012) is single-tenant at launch and multi-tenant in Tier 1 hosted. The architectural question this ADR resolves is what auth and tenancy primitives ship on day one so the Tier 1 transition is a configuration change rather than a refactor.

The threat model at launch is constrained: a single user, two machines, on Tailscale or LAN. Tailscale handles network-level access control. The gateway provides tenant-scoped operation primitives; it does not enforce tenant identity against attackers at launch. This is a deliberate scoping choice — over-engineering attacker-grade isolation for a single-user pre-launch deployment is wasted work, but shipping without the seam means rebuilding auth and storage when Tier 1 lands.

Two design dimensions need resolution:

**Token format.** The gateway needs a way for the Pane client to authenticate. The two viable choices are JSON Web Tokens (JWT, signed and self-contained) and opaque bearer tokens (random bytes, gateway-resolved server-side). Each has tradeoffs that change at single-tenant vs multi-tenant scale.

**Tenancy primitives.** Whatever shape "a tenant" takes in Tier 1 needs to exist as a column on every gateway state row at launch, or the upgrade is a data migration plus a code rewrite. The question is whether to make `tenant_id` a real value flowing through every query or to add it later when the multi-tenant code lands.

A third practical concern: the gateway holds a credential to authenticate to OC on the user's behalf. In Tier 1, that becomes a per-tenant OC credential (different users running different OC instances). At launch, it's a single OC token in config. The gateway↔OC auth path needs to accommodate both shapes without rework.

### What if we didn't decide

Two predictable failure modes:

- **JWT-by-default.** Pick JWT for "industry standard" reasons without examining single-tenant cost. JWT requires signing key infrastructure, key rotation policy, claim schema, library dependency, and revocation strategy (typically a denylist or short expiry + refresh tokens). All of that lands at launch, for a deployment with one user and one token. When Tier 1 ships, JWT is reused — fine — but the launch cost is real and didn't earn anything.
- **No tenancy seam at launch.** Defer `tenant_id` as a "we'll add it when we need it" move. Tier 1 then becomes: add `tenant_id` to N tables, rewrite every query, audit every endpoint handler, write a data migration, hope nothing leaks. The seam is cheap to add at the start (one column, one auth-context field) and expensive to retrofit later.

## Decision

### Token format: opaque bearer tokens at launch

The gateway issues 256-bit opaque bearer tokens during pairing. Tokens are stored server-side in a `tokens` table; resolution is a single indexed lookup per request. No JWT, no signing key, no claims.

The pairing flow:

1. Gateway started on the OC machine. On first run, generates a 6-digit pairing code (5-min expiry) and the TLS cert fingerprint. Both displayed.
2. User enters URL + code + fingerprint into Pane.
3. Pane exchanges the code for an opaque token via `POST /pair`. Token is long-lived (90 days, auto-rotated 7 days before expiry).
4. Pane stores token + fingerprint. All subsequent calls use `Authorization: Bearer <token>` with the pinned cert fingerprint.

JWT can be introduced post-launch with a `token_type` discriminator on the `tokens` table. Existing opaque tokens stay valid; new tokens issued in a JWT-aware build are signed. No forced rotation.

### Tenancy primitives: structural seam at launch

Every gateway state table carries a `tenant_id` column. At launch, the value is always `default`. There is exactly one tenant per gateway deployment.

Tables in scope at launch:

- `tokens` — `tenant_id` resolves from token lookup
- `tenant_agents` — `(tenant_id, oc_agent_id)` is the routing key
- `sync_queue` (gateway-side) — every queued item is tenant-scoped
- `sync_pending` (gateway-side) — every pending change is tenant-scoped
- `sessions` — every session belongs to a tenant
- `tenant_oc_credentials` — `(tenant_id) → OC bearer token`. At launch, exactly one row.
- `tenant_quotas` — schema present, empty at launch. Per-tenant rate limits, max sessions, max agents. Endpoint middleware checks quotas; with empty table, middleware short-circuits to no-op.

Every endpoint handler reads `tenant_id` from the auth context (the result of token resolution) and passes it to every query. There are no global queries — no "give me all sessions," only "give me all sessions for tenant X." This holds at single-tenant launch where `X` is always `default`.

Tier 1 flip introduces real tenant resolution from token (currently always `default`), multi-tenant pairing UI, OC connection pooling per tenant, and population of `tenant_quotas`. None of these touch the endpoint handlers — they all already pass `tenant_id` through.

### Gateway → OC auth: per-tenant credential lookup with single-row launch

The gateway authenticates to OC as the user. At launch the gateway holds one OC bearer token in `tenant_oc_credentials` keyed by `tenant_id = 'default'`. The WS handshake to OC includes `Authorization: Bearer <oc_token>` looked up from this table, scoped by the tenant context of the originating request.

In Tier 1 the same table holds N rows, one per tenant, with user-supplied OC tokens. Gateway never proxies user-side OC tokens to other tenants. Connection state is partitioned per `tenant_id`.

### Threat-model honesty

The launch threat model is "single user, two machines, Tailscale or LAN." That is what this auth design defends against — wire interception (via TLS), token theft (via cert fingerprint pinning), and accidental cross-tenant leakage (via structural tenancy). It is not what this design defends against in Tier 1: hostile attackers, token-format attacks, or attackers with stolen network-layer access.

Tier 1 hardening is documented as required before hosted launch. This ADR does not pre-commit those hardening choices — it commits to keeping the seam shape compatible with them.

## Consequences

### Positive

- **Single-table token resolution.** No signing keys, no claims schema, no JWT library at launch. One indexed lookup per request. Lower attack surface (no algorithm-confusion class of bug).
- **Trivial revocation.** Delete the row. No denylist, no short-expiry-plus-refresh dance. Important for the pairing UX where re-pair is the recovery path for a compromised token.
- **Tier 1 transition is configuration, not refactor.** The same handlers, queries, and tables work at N tenants as at 1. The work is real-tenant-resolution, multi-tenant-pairing-UI, and credential-population — not query rewrites.
- **OC credential per tenant from day one.** Eliminates the launch path where one global OC credential lives in config and Tier 1 has to migrate to per-tenant lookup.
- **Honest about scope.** Threat model is explicit: pre-launch is Tailscale/LAN, single-user. We are not pretending otherwise. This makes Tier 1's hardening requirements concrete rather than aspirational.
- **Quota seam present.** Per-tenant quotas (rate limit, max sessions, max agents) are spec'd as schema-only at launch. When Tier 1 needs them, they populate; middleware already runs the check.

### Negative

- **Server-side token state required.** Opaque tokens force the gateway to maintain a `tokens` table with read-on-every-request. JWT would be stateless in that respect. At launch this is one user, one token, microsecond lookup against an in-memory or SQLite-backed store; the cost is real but small. At Tier 1 scale, token-table read becomes a hot path that needs caching or scaling — this is the first place the design will need attention when load grows.
- **`tenant_id = 'default'` in every query at launch is dead code from the launch user's perspective.** Every query carries an extra clause that always matches. Performance impact is negligible (indexed lookup), readability cost is small (one extra clause), but it's overhead that earns its keep only at Tier 1.
- **Pairing UX adds steps.** URL + 6-digit code + cert fingerprint is more than one-click. Mitigation: it's a one-time operation, and the alternative (no pairing) makes split-machine onboarding undefined.
- **Self-signed cert + fingerprint pinning is unfamiliar UX.** Users don't see a green-padlock; they see a fingerprint string they're asked to verify. Acceptable at launch (Tailscale users are technical), revisited as a §22C.20 spike #2 for whether Let's Encrypt support belongs at launch or post-launch.

### Risks

- **JWT introduction later may want claim shapes that require token re-issuance.** If Tier 1 wants claims (tenant_id baked in, role, quota tier) the migration is "issue new JWT-format tokens, leave opaque tokens valid until expiry." 90-day token lifetime caps the migration window. Risk is bounded.
- **Token rotation can fail silently if Pane is offline at the rotation moment.** Mitigation: rotation window is 7 days before expiry, gateway accepts both old and new tokens during the window, expiry re-pair is the documented fallback.
- **Per-tenant OC credential table is one-row at launch.** If launch ships and Tier 1 doesn't ship for many months, the table stays one-row and the per-tenant abstraction is paper-only for that period. The risk is forgetting why the table is shaped that way and "simplifying" it during a refactor. Mitigation: this ADR + INVARIANT #5 (cross-tenant access impossible by construction) document the intent.
- **Single OC credential at launch means a token leak compromises everything Pane can do via the gateway.** Mitigation: the OC credential never leaves the gateway machine; it is not transmitted to Pane. The same trust property the bridge plugin had today, carried forward.

## Alternatives Considered

**JWT at launch.** Self-contained tokens, no server-side lookup, industry standard. Rejected for launch: the cost (signing key infrastructure, claim schema, library dep, revocation strategy via denylist or short-expiry-plus-refresh) doesn't earn its keep at one user with one token. JWT can be introduced post-launch via a `token_type` discriminator without invalidating existing opaque tokens. The decision is not opaque-vs-JWT-forever; it's opaque-first, JWT-when-warranted.

**No tenancy seam at launch.** Skip `tenant_id` columns entirely; add them when Tier 1 lands. Rejected: the migration is the whole point of having a seam. Adding `tenant_id` to a working multi-table system, rewriting every query, auditing every handler, writing a data migration — that is the refactor this ADR exists to prevent. The cost of the seam at launch is one column per table and one field on auth context. The cost of retrofitting is the entire gateway service.

**Mutual TLS instead of bearer tokens.** Pane authenticates with a client certificate; no token at all. Rejected: certificate management UX is significantly worse than 6-digit pairing codes for non-enterprise users; revocation requires CRL or OCSP infrastructure; the seam to multi-tenant is harder (per-tenant CA or per-tenant cert issuance) than `tenant_id` on a token row. Worth revisiting if Tier 1 sells into enterprises that demand mTLS.

**API keys with per-key scopes (instead of bearer-token-with-tenant-context).** Each scope is a separate token (sync, models, sessions, etc.). Rejected as over-engineering for launch. Pane is the only client; it always wants every scope. Scoped tokens become useful when third-party integrations exist, which is not a launch concern. Adding scopes later is a `token_scopes` column on `tokens`; the seam is straightforward to retrofit because token resolution is already centralized.

**Tailscale-only deployment, no in-app auth.** Rely on Tailscale ACLs entirely. Rejected: makes Tailscale a hard product dependency, fails any non-Tailscale deployment, fails Tier 1 entirely. The seam stays even if every launch user happens to use Tailscale.

**Static tenant_id in code, no column.** Hardcode `tenant_id = 'default'` everywhere; promote to column when Tier 1 lands. Rejected: indistinguishable from "no tenancy seam at launch" once you trace through the implications. Either the queries are tenant-scoped or they aren't; there is no middle ground that doesn't reduce to a refactor.

## References

- Spec section: `docs/spec/sync.md` §22C.5 (auth and multi-tenancy seam — full implementation detail), §22C.6.1 (pairing endpoint), §22C.13 (payload limits — interacts with quota middleware)
- Related ADRs: ADR-0012 (Pane Gateway as primary sync transport — establishes the gateway service this ADR's auth model lives inside); ADR-0014 (bridge vs Supabase split — clarifies what auth model the user-data path uses, which is unaffected); ADR-0015 (gateway↔OC transport — the path that uses `tenant_oc_credentials`)
- INVARIANTS to file alongside this ADR (subset of §22C.18): #5 (cross-tenant access impossible by construction), #6 (bearer token is opaque and gateway-resolved at launch)
- Audit reference: 2026-04-27 bridge audit (#689) gap #11 (plaintext HTTP) — auth design depends on TLS termination, which ADR-0012 establishes architecturally
- Tier 1 hardening dependencies (not committed in this ADR, flagged for follow-up): real tenant resolution from token, multi-tenant pairing UI, JWT introduction if claim-based authorization is needed, OC connection pooling per tenant
