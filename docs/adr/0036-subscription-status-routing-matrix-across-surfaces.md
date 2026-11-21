# ADR-0036: Subscription Status Routing Matrix Across Pane Surfaces

**Status:** Retired — Superseded by ADR-0070 (2026-11-09)
**Superseded by:** ADR-0070. Pane no longer consumes `subscription_status` on any surface; S7, the cross-surface routing matrix and the post-take-over entitlement check are removed.
**Date:** 2026-05-10
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0029 (extends the 5-value enum's UI mapping to a cross-surface invariant), ADR-0035 (composes with surface-discriminated active-machine mismatch handling), ADR-0025 (adds a third entitlement call site beyond launch + on-focus)
**Related:** ADR-0027 §4 (take-over endpoint contract — explicitly not superseded by ADR-0029), ADR-0024 (local-only decryption)

## Context

ADR-0029 widened `subscription_status` from a 3-value enum (`active`,
`lapsed`, `cancelled`) to a 5-value enum (`active`, `past_due`,
`unpaid`, `cancel_at_period_end`, `canceled`) on the entitlement
response. ADR-0029 superseded ADR-0027 §2 (sign-in response) and §3
(entitlement response) — but explicitly did **not** supersede §4
(take-over endpoint).

The take-over endpoint (`POST /v1/account/active-machine`) returns
the original 3-value error vocabulary: 403 `subscription_lapsed` and
403 `subscription_cancelled`. Per the wrapper source-read in
ecom-backend #21, the wrapper denormalizes the 5-value enum to those
two codes plus a 200 allow-through for both `active` and
`cancel_at_period_end`. The wrapper's behavior on this endpoint is
the locked artifact ADR-0029 chose not to disturb.

This produces a routing question Pane has not pinned: what does each
of the 5 statuses do on each of Pane's entitlement-observing
surfaces? The original §6.1.2 / §6.1.3 prose — "subscription_status
≠ active → S7" — was written for the 3-value world. Applied verbatim
against the 5-value enum it produces ambiguity:

- The take-over POST returns 200 for `cancel_at_period_end`, but
  §6.1.2's blanket "≠ active → S7" routes that same status to a
  hard-block on the next entitlement check. The wrapper is
  permissive; Pane is strict. Read in isolation, the two surfaces
  appear to give the same customer two answers.
- The take-over POST 403 codes don't tell Pane which CTA to render
  on S7. The S7 redesign exposes three CTA states (Update Payment /
  Keep Subscription / Resubscribe). The wrapper's 3-value error
  vocabulary collapses two of those distinctions:
  `subscription_lapsed` covers both `past_due` and `unpaid`;
  `subscription_cancelled` covers `canceled` (not
  `cancel_at_period_end`, which never reaches the 403 surface). The
  mapping needs explicit pinning so the implementer doesn't guess.
- ADR-0025 defines two entitlement call sites (launch + on-focus).
  Neither fires immediately after a take-over 200. A
  `cancel_at_period_end` customer who taps [Take over here] would
  therefore land on Home after the 200 and stay there until the next
  launch or backgrounding-then-focus event. The wrapper's
  `cancel_at_period_end` allow-through becomes an exploitable
  loophole rather than a brief transition; Decision 1's hard-block
  routing has no surface to fire on until the customer happens to
  background the app.

ADR-0035 (active-machine mismatch call-site rule) introduced surface
discrimination for the orthogonal mismatch question. Subscription-
status routing is also surface-observed (sign-in response,
cached-token entitlement check, take-over response, on-focus
entitlement check). This ADR pins how each surface handles each
status, composing with — not contradicting — ADR-0035's mismatch
discrimination.

**Scope of this ADR (what's actually new).** ADR-0029 §2.1 already
pinned the per-status UI mapping. ADR-0036 formalizes (1) the
cross-surface invariant across all entitlement-bearing surfaces, (2)
the take-over POST 403 mapping into that matrix, (3) a new third
entitlement call site that fires immediately after a take-over 200
and closes the wrapper's `cancel_at_period_end` allow-through, and
(4) the take-over 200 reframing as machine-claim signal rather than
entitlement. Future readers loading ADR-0036 alone need this list to
know what changed against §6.1.2 and §6.1.3 as they previously stood.

### What if we didn't decide

- The implementer guesses the CTA mapping for the take-over 403 path
  and ships a UX where, e.g., `subscription_lapsed` always renders
  the generic "Update Payment" CTA without an explicit forward
  pointer to the 5-value matrix that supplied the choice — a silent
  drift against ADR-0029's three-CTA design.
- The wrapper-permissive `cancel_at_period_end` take-over allow-
  through stays a real loophole — customers in that status take over
  a machine and use Pane on Home until they happen to background and
  refocus. Pane's hard-block routing exists in §6.1.2 but has no
  surface to fire on between the 200 and the next ADR-0025 trigger.
- Future surface additions (a Settings entitlement panel, a
  diagnostics view) re-derive the routing ad-hoc and drift from
  §6.1.2.

## Decision

### 1. Status routing is cross-surface invariant

For any surface that observes `subscription_status` — sign-in
response, on-launch cached-token entitlement check, on-focus
entitlement check, post-take-over entitlement check (Decision 3) —
the routing decision is identical:

| `subscription_status` | UI |
|---|---|
| `active` (no `subscription_action_url`) | Allow access |
| `active` + `subscription_action_url` non-null | Allow access; surface grace-period soft-block toast (§6.1.2.1) |
| `past_due` | S7 hard-block, "Update Payment" CTA |
| `unpaid` | S7 hard-block, "Update Payment" CTA |
| `cancel_at_period_end` | S7 hard-block, "Keep Subscription" CTA |
| `canceled` | S7 hard-block, "Resubscribe" CTA |

This matches ADR-0029 §2.1 and §2.3 verbatim. This ADR's contribution
is the cross-surface invariant: the same status value produces the
same UI regardless of which API call returned it. The matrix is the
canonical source of truth; surfaces must not derive divergent UI for
the same status.

### 2. Take-over POST 403 mapping into the matrix

The wrapper's take-over endpoint (ADR-0027 §4, not superseded by
ADR-0029) returns 3-value-era error codes. Per ecom-backend #21
source-read, the wrapper denormalizes:

| Wrapper response | Source `subscription_status` (wrapper-side) |
|---|---|
| 200 | `active`, `cancel_at_period_end` |
| 403 `subscription_lapsed` | `past_due`, `unpaid` |
| 403 `subscription_cancelled` | `canceled` |

Pane interprets the take-over response through Decision 1's matrix:

- **200:** take-over succeeded. The post-take-over entitlement check
  (Decision 3) fetches the canonical `subscription_status` and
  applies Decision 1. For `cancel_at_period_end`, the check routes to
  S7 (the customer does not land on Home); for `active`, the check
  advances to Home.
- **403 `subscription_lapsed`:** route to S7 with "Update Payment"
  CTA. The wrapper's coarser code does not distinguish `past_due`
  from `unpaid`; both share the same CTA in Decision 1, so no
  information is lost on the Pane side.
- **403 `subscription_cancelled`:** route to S7 with "Resubscribe"
  CTA. The wrapper does not surface `cancel_at_period_end` as a 403,
  so this code unambiguously corresponds to `canceled`.
- **Other 4xx/5xx:** surface "Couldn't take over — try again" with
  manual retry. No state change.

### 3. Pane runs an entitlement check immediately after take-over

ADR-0025 defines two call sites for `GET /v1/account/entitlement`:
launch and on-focus. A 200 response from
`POST /v1/account/active-machine` is a third call site, defined here:

**Pane runs `GET /v1/account/entitlement` immediately after
`POST /v1/account/active-machine` returns 200, in addition to
ADR-0025's launch + on-focus cadence.** Routing to onboarding-resume
/ Home is gated on the entitlement check's outcome; the take-over
200 alone does not advance the UI past the take-over flow.

Without this third call site, a customer in `cancel_at_period_end`
who taps [Take over here] gets 200, lands on Home, and stays there
until the next launch or backgrounding-then-focus event. The
wrapper's `cancel_at_period_end` allow-through (Decision 2) becomes
an exploitable loophole rather than a brief transition, because
Decision 1's hard-block routing has no surface to fire on between
the 200 and the next ADR-0025 trigger.

The post-take-over check uses the same `GET /v1/account/entitlement`
call shape as the other two call sites and feeds Decision 1's matrix
the same way they do. It is not a new wrapper contract — only a new
Pane-side trigger for an existing call. Implementation: the
take-over success handler chains directly into the entitlement check
before any UI advance.

### 4. The take-over POST 200 is not entitlement

A 200 from `POST /v1/account/active-machine` confirms only that the
wrapper has claimed `active_machine_id` for this install. It does
not entitle the customer to use Pane. Pane treats the 200 as a
machine-claim signal; the post-take-over entitlement check
(Decision 3) is the source of truth on whether the customer sees
Home or S7.

This makes the wrapper's `cancel_at_period_end` allow-through
consistent with Pane's hard-block: the wrapper allows the machine
claim; Pane gates access on the entitlement check that immediately
follows. Same customer, same status, same UI on either surface — no
double answer.

### 5. Composition with ADR-0035

ADR-0035 discriminates how active-machine mismatch is handled across
the sign-in surface and the cached-token surface. This ADR's matrix
applies independently of that discrimination: the subscription gate
runs before the machine gate on every entitlement-bearing surface
(per existing §6.1.3 precedence — "subscription gate takes
precedence over machine check"). Decision 1 above governs the
subscription gate; ADR-0035 governs the mismatch gate downstream of
it. The two decisions compose by ordering, not by overriding.

The post-take-over call site (Decision 3) composes the same way the
other two ADR-0025 call sites do. On a fresh take-over the mismatch
gate is degenerate (Pane just claimed `active_machine_id`, so the
response should match the local `install_uuid`), but the
discrimination still applies; if a race produces a mismatch (another
machine claimed in the same window), ADR-0035's surface rules
govern the response.

## Consequences

### Positive

- **Single source of truth for status → UI.** Future surfaces
  (Settings entitlement panel, error-state diagnostics, telemetry
  views) refer to Decision 1's matrix.
- **Take-over CTA mapping is unambiguous.** The wrapper's 3-value
  error vocabulary maps cleanly onto Pane's 5-value enum without
  loss for the routes it covers.
- **Wrapper contract stays untouched.** ADR-0027 §4 remains valid.
  The wrapper's denormalization is documented but not changed.
  ecom-backend #21 closes on the Pane side without requiring wrapper
  changes.
- **`cancel_at_period_end` loophole closed.** Decision 3's
  post-take-over check fires before any UI advance, so the customer
  never reaches Home in that state.
- **Cross-surface invariant catches regressions.** Future spec
  edits that introduce a new entitlement-observing surface (or a
  new status enum value) must pin routing in Decision 1. §11
  audits catch divergence.

### Negative

- **`cancel_at_period_end` produces a brief delay before S7.**
  Customer in `cancel_at_period_end` who taps [Take over here] sees
  the take-over POST round-trip plus the post-take-over entitlement
  check round-trip (Decision 3) before S7 renders. Sub-second on a
  normal connection, but two sequential calls. A wrapper contract
  change to 403 take-over for this status would collapse to a single
  round-trip — see "Findings" below.
- **Two error vocabularies live in §6.1.3.** The take-over table
  shows wrapper 3-value codes alongside §6.1.2's 5-value matrix.
  Readability cost. Mitigation: the explicit denormalization sub-
  table in Decision 2 above; cross-references between §6.1.2 and
  §6.1.3.

### Risks

- **Wrapper denormalization drift.** If the wrapper ever changes how
  it maps Stripe states to take-over 403 codes (for example, starts
  returning `subscription_lapsed` for `cancel_at_period_end`),
  Decision 2's mapping silently breaks. Mitigation: the
  denormalization is wrapper-side per ADR-0027 §4; ecom-backend
  repo's integration tests should cover the mapping against Stripe
  test fixtures. Pane-side detection: §11 audit on the
  `/v1/account/active-machine` contract.
- **A new Stripe state Pane's enum doesn't anticipate.** ADR-0029
  Risk already documents this. If the wrapper falls through to
  `canceled` as a default, Decision 1 routes to S7 with
  "Resubscribe" — likely-but-not-always-correct. Pane-side
  detection out of scope; wrapper-side mapping policy (filed as a
  follow-up by ADR-0029) is the catch-net.
- **Post-take-over check fails or times out.** The take-over POST
  returned 200 but the immediate entitlement check 5xxs or hits a
  network drop. Per ADR-0024/0025, Pane continues with last-known
  entitlement on transient ecom unreachability — that means routing
  off the cached pre-take-over `subscription_status`, which is
  correct (the take-over didn't change subscription state). The
  next ADR-0025 trigger picks up the canonical state. Documented
  here so it isn't surprising at implementation time; not a separate
  decision.

## Alternatives Considered

**Treat `cancel_at_period_end` as a soft-block (banner, not S7).**
Customer keeps Pane access through period end with a "Subscription
canceled. Access ends [date]. [Keep Subscription]" banner. Matches
ADR-0029's "still has access through the period end" prose and the
wrapper's take-over allow-through. Eliminates the take-over delay
described under Consequences. **Rejected** because ADR-0029 §2.1
explicitly maps `cancel_at_period_end` to S7 hard-block with "Keep
Subscription" CTA, and the directive for this session is to
supplement ADR-0029, not supersede it. The "still has access" prose
refers to billing state (Stripe still considers the subscription
active and bills accordingly); ADR-0029's UI design is for Pane to
hard-block on cancel-pending and prompt the customer to undo. If the
design intent on `cancel_at_period_end` is reopened, the path is a
clarifying-supersession ADR against §2.1's row, not a silent edit.

**Have the wrapper return 403 `subscription_cancel_pending` for
take-over on `cancel_at_period_end`.** Eliminates the take-over 200
→ post-check S7 delay. **Rejected as out of scope.** This is a
contract change to ADR-0027 §4, which ADR-0029 explicitly did not
supersede. Filed as a wrapper-side finding (References).

**Have Pane gate the take-over CTA itself by checking
`subscription_status` from the cached entitlement before issuing the
POST.** Customer in `cancel_at_period_end` would see S7 instead of
S8 in the first place, never reaching the take-over POST.
**Rejected** because (a) it conflates the machine-claim and
subscription gates (the cached `subscription_status` could race
against a mid-flight Stripe transition; the customer might be
legitimately taking over right after recovering subscription state),
and (b) it puts wrapper-side allow-list logic in Pane, which is
exactly what ADR-0029 Decision 2 declines to do (the wrapper owns
status denormalization; Pane reads).

**Skip Decision 3 — let the next ADR-0025 trigger pick up
`cancel_at_period_end` on its own.** Simpler to specify. **Rejected**
because it leaves the loophole described in Context: a customer in
`cancel_at_period_end` lands on Home after take-over and uses Pane
unblocked until they happen to background the app. The whole point
of the routing matrix is no surface gives the same customer two
answers; without Decision 3, the take-over surface gives a permissive
answer for an arbitrary window of wall-clock time.

**Pin the routing matrix in §6.1.2 only, no new ADR.** The matrix
already lives in §6.1.2's prose post-amendment. **Rejected** because
the cross-surface invariant, the take-over POST 403 mapping, and the
post-take-over call site are load-bearing decisions across multiple
surfaces and across the take-over contract; they merit a dedicated
ADR file for audit traceability. Future readers of ADR-0027 §4
(unsuperseded) and ADR-0025 (cadence definition) need a forward
pointer to Decisions 2 and 3 respectively; that pointer lives here.

## References

- ADR-0029 (Entitlement Response Shape and Email-Check Privacy
  Posture) — defines the 5-value enum and the per-status UI mapping
  this ADR extends to cross-surface invariance.
- ADR-0035 (Active-Machine Mismatch Call-Site Rule) — composes with
  this ADR on the entitlement-check surface; mismatch discrimination
  applies after the subscription gate, on all three call sites
  (launch, on-focus, post-take-over).
- ADR-0027 §4 (Take-over while lapsed: 403, fail fast) — explicitly
  not superseded by ADR-0029. Remains the source of truth for
  wrapper-side take-over response shape; this ADR pins the Pane-side
  interpretation of those response codes.
- ADR-0024 (Local-only Decryption) — context for what S7 hard-block
  means in practice (UI overlay, not data lock).
- ADR-0025 (Ecom Check Cadence: launch + on-focus) — the two call
  sites this ADR extends with a third (Decision 3, post-take-over).
  ADR-0025's status banner carries a forward pointer to this ADR.
- `docs/spec/onboarding.md` §6.1.2 — amended by this ADR.
- `docs/spec/onboarding.md` §6.1.3 — take-over POST table amended by
  this ADR; mismatch handling preserved per ADR-0035.
- ecom-backend #21 — wrapper take-over source-read; closes on the
  Pane side when Decisions 2 and 3 land in §6.1.2 and §6.1.3.
- Issue #973 — Pane-side single-active-machine implementation.

### Wrapper-side findings (not drafted by this ADR)

1. **`cancel_at_period_end` take-over fail-fast.** The wrapper
   currently returns 200 for take-over on `cancel_at_period_end`,
   producing a sub-second delay between the take-over 200 and the
   post-take-over entitlement check that routes to S7 (Decision 3).
   A wrapper-side change to 403 (e.g., a new
   `subscription_cancel_pending` code) on take-over for this status
   would collapse the two round-trips to one and bring the take-over
   surface into line with the entitlement surface immediately. This
   is an ADR-0027 §4 contract change and is out of scope for this
   ADR; filed as a candidate follow-up wrapper-side ADR.

2. **Wrapper Stripe-state denormalization policy.** Already filed as
   a follow-up by ADR-0029. Not new here, but Decision 2 of this ADR
   depends on the denormalization being stable; the wrapper-side
   policy ADR remains the catch-net for drift.
