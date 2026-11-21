# ADR-0035: Active-Machine Mismatch — Call-Site Rule for UI Disposition

**Status:** Accepted — Superseded in part by ADR-0070 (2026-11-09)
**Superseded by:** ADR-0070 for the mismatch rule itself: both surfaces, the discriminator, S8 and silent sign-out are removed. What stands is the §6.1.5 principle this ADR leaned on — `clear_auth_session` is the only path to a clean no-session state, and it is called only on the flag-on account path (ADR-0070 Decision 5).
**Date:** 2026-05-10
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0024 (Local-Only Decryption), ADR-0025 (Ecom Check Cadence — Launch + On-Focus), ADR-0026 (Install UUID for `active_machine_id` Matching)

## Context

`docs/spec/onboarding.md` §6.1.3 (Single-active-machine check) specifies two contradictory behaviors for the same wire signal — an entitlement response with `active_machine_id` ≠ local `install_uuid`:

1. The decision table's third row routes mismatch unconditionally to S8 take-over UI.
2. A prose paragraph in the same subsection ("The displaced machine notices on its next entitlement check … and silently signs out per the redesign edge case spec") routes mismatch to silent sign-out.

The contradiction reflects a real design tension that wasn't pinned in the spec rewrite that produced §6.1.3. `Pane_Onboarding_Redesign_Decisions__2026-05-06.md` § "State machine: take-over" and § "S8" locked silent sign-out for the displaced machine explicitly, citing the rationale formalized later in ADR-0026: a customer with a desktop and a laptop legitimately switching between them should not see S8 prompts on each launch, because each take-over evicts the other machine and the loop bounces the seat indefinitely. ADR-0025 § "Behavior on each check" alludes to both behaviors ("Pane surfaces S8 take-over UX or — if take-over has already been confirmed elsewhere — silent sign-out + return to S1") but defers the discriminator.

Pane has no way to distinguish "displaced — was active, now isn't" from "fresh sign-in / new install — never was active" on the wire signal alone. Both produce the same `active_machine_id` ≠ `install_uuid` response. The discriminator must come from Pane's local context.

### What if we didn't decide

- **Implementation picks one reading silently.** Issue #973's work would have to choose; whichever path lands first becomes the de facto rule, and the other behavior gets dropped without an explicit decision. PR review catches it only if the diff makes the choice visible — which a single-line `if` in a route handler often doesn't.
- **The contradiction surfaces as a customer-support issue post-launch.** A desktop+laptop customer hits ping-pong friction (universal-S8 reading) or a migrating customer can't take over their own seat (universal-silent-sign-out reading). Either is a recoverable bug; neither is the kind of thing we should be shipping to find out which one Stia picked.
- **The wire-vs-local-state distinction stays implicit.** Pane's broader pattern is that ecom is the source of truth for entitlement and Pane's local state is incidental. This ADR pins the one place where local state is load-bearing — the surface that produced the entitlement snapshot carries information the wire response does not.

## Decision

**Pane discriminates `active_machine_id`-mismatch UI disposition by the surface that produced the entitlement snapshot, not by the wire signal alone.**

Two surfaces consume `active_machine_id`:

1. **Sign-in surface.** Inline with the entitlement snapshot embedded in `POST /v1/auth/signin` responses, per `docs/spec-prep/2026-05-07-pane-ecom-contract.md` §2 and ADR-0029. Not a cadence event; consumed once per sign-in attempt, before any session token is in keychain.
2. **Cached-token surface.** `GET /v1/account/entitlement` calls, fired at the two cadence call sites defined by ADR-0025 (at launch with a cached token loaded from keychain; on window focus after backgrounding). Both ADR-0025 call sites collapse to the same UI disposition because they share the same precondition — a cached token, which by construction means a session was previously established on this install.

On `active_machine_id` ≠ local `install_uuid`:

- **Sign-in surface** → Pane shows S8 take-over UI. The customer has explicit agency to claim the seat for this install.
- **Cached-token surface** → Pane silently signs out: routes to S1 on next launch, with keychain and session metadata cleared via `clear_auth_session` per `docs/spec/onboarding.md` §6.1.5 (which already lists "take-over completion on the displaced machine" among `clear_auth_session`'s callers). No UI prompt and no "you were signed out" toast.

Subscription-gate routing (`subscription_status` ≠ `active` → S7) and match cases (continue) are unchanged across both surfaces.

The surface identity is the discriminator. `POST /v1/auth/signin` is consumed only as part of an explicit sign-in attempt; `GET /v1/account/entitlement` is consumed only with a previously-cached token, which by construction means the install was previously active (auto-claimed at first sign-in or take-over-confirmed at S8). The surface carries the "have I ever been active" semantics implicitly via which endpoint produced the signal.

S8 cancel (the customer dismisses the take-over UI without confirming) clears the just-cached session via `clear_auth_session` and returns to S1, per §6.1.5's existing contract for that function. This prevents a cancelled install from later being mistaken for "previously active" at the cached-token surface — a cancelled install never reaches that surface, because its keychain is clean.

Silent sign-out fires only on a successful 200 response from `GET /v1/account/entitlement` that carries a mismatched `active_machine_id`. Network failures and 5xx responses preserve last-known entitlement per ADR-0024 / ADR-0025; they do not trigger silent sign-out.

## Consequences

### Positive

- **Ping-pong friction is structurally prevented.** A customer alternating between desktop and laptop sees S8 only once per migration (when re-signing in on the other machine); the displaced machine signs itself out silently on the next launch or focus. ADR-0026's rationale is preserved without this ADR re-litigating it.
- **Customer agency is preserved at the migration moment.** A customer reinstalling on a new machine, or moving from a lost laptop to a replacement, sees S8 at sign-in time and can take over their seat with one tap. Universal-silent-sign-out would have stripped this path.
- **The discriminator is implementation-cheap.** Pane already routes sign-in and cached-token entitlement calls through different surfaces — the UI disposition can be attached to the surface without introducing a new boolean or persisted "previously active" flag.
- **The sibling-service contract is unchanged.** ADR-0027 and ADR-0029 stay locked. The wrapper (`pane-ecom-backend`) does not need to expose a "displaced vs fresh" signal; the existing 200 response with `active_machine_id` is sufficient.
- **The rule is anchored to landed spec.** §6.1.5's `clear_auth_session` contract (BUILT per PR #1033) already lists the displaced-machine silent-sign-out path and the S8 Cancel path as callers. This ADR operationalizes the disposition rule that determines *when* `clear_auth_session` is invoked from the cached-token surface; §6.1.5 owns *how*. The two specs read together coherently.

### Negative

- **Subtle invariant if `POST /v1/auth/signin` is ever called with a token already in keychain.** The current flow assumes no cached token (Pane is at S1) when sign-in fires. Today this holds because the only paths into S1 — `clear_auth_session`'s callers per §6.1.5 — all clean keychain before returning the user to sign-in. If a future code path calls sign-in re-entrantly while a cached token exists, the discriminator could fire S8 on what is structurally a cached-token state. Mitigation: any future sign-in surface (Settings re-auth, token-rotation prompt, etc.) must clear keychain before calling `POST /v1/auth/signin`, by routing through `clear_auth_session` first. §6.1.5's "Any state-recovery path that needs a clean S1 entry" clause already covers this; future spec sections that add non-S1 sign-in paths must call it out explicitly.
- **The "displaced" semantics are not visible on the wire.** A future ecom-side observer (analytics, support tooling) cannot tell from the API call alone whether a given mismatch produced an S8 prompt or a silent sign-out on the Pane side. Acceptable: the wrapper does not need to know, and Pane-side telemetry (if added later, post-launch) can carry the disposition separately.
- **Silent sign-out is, by name, silent.** A customer whose displaced machine signs itself out gets no in-app indication; on next launch they see S1 and may briefly wonder why they're signed out. The redesign-decisions doc accepts this trade explicitly — files on disk are unaffected (ADR-0024) and the customer has just signed in elsewhere, so the cause is generally obvious. Mitigation candidates (one-time dismissible toast on next launch, account-section banner) are post-launch UX polish; v1 ships silent.
- **The "surface is the discriminator" rule is not self-documenting in code.** A reader of the cached-token call site sees a `GET /v1/account/entitlement` and a 200 response with mismatch; the silent-sign-out branch needs an inline comment citing this ADR or the rule re-fragments. Mitigation: §6.1.3 carries the rule in spec form; PR review on #973 should require an inline citation to ADR-0035 at the silent-sign-out branch.

### Risks

- **A wrapper bug returning a stale `active_machine_id` on `GET /v1/account/entitlement` could silently sign out the *current* active machine.** Mitigation: take-over via `POST /v1/account/active-machine` is synchronous and last-write-wins per ADR-0027 §4; the wrapper should not return a stale `active_machine_id` for any window where Pane could legitimately be displaced. If the wrapper exhibits stale-read behavior under load post-launch, surface it as a wrapper bug, not a Pane-side fix.
- **A customer who legitimately wants to take over from the displaced machine has to actively re-sign-in.** This is the intended behavior, but worth naming. After silent sign-out, the customer lands at S1 on next launch and signs back in to trigger S8. A one-tap "take it back" affordance on the displaced install would be a UX simplification for this case; filed as post-launch.
- **The supersession-vs-supplement distinction.** This ADR explicitly does not supersede ADR-0026 — its ping-pong-friction rationale is operationalized here, not overturned. Future readers loading ADR-0026 in isolation see the rationale; loading ADR-0035 in isolation sees the operationalization with ADR-0026 cited. The two read together. The "Supplements" relationship is documented in this ADR's header per the existing precedent (ADR-0028 / ADR-0027, ADR-0029 / ADR-0027, ADR-0034 / ADR-0031).

## Alternatives Considered

**Option 1: Universal S8 on any mismatch.** Every entitlement check, regardless of surface, surfaces S8 on mismatch. **Rejected** because it reproduces the ping-pong-friction failure ADR-0026 identifies as the primary threat. The customer-fatigue threat dominates the malicious-actor threat for v1, and gateway-side single-Pane pairing rejection (sub-task #7) holds the line on OC paths regardless. Universal S8 buys Pane nothing the gateway doesn't already enforce more strongly, while breaking the legitimate desktop+laptop customer.

**Option 2: Universal silent sign-out on any mismatch.** Every mismatch silently signs out, including at sign-in. **Rejected** because it strips the migration path — a customer reinstalling on a new machine or replacing a lost laptop signs in, hits silent sign-out, and is bounced back to S1 with no UI affordance to claim the seat. The take-over UI exists *because* migration is the legitimate flow ADR-0026 designs for; removing the surface defeats the purpose.

**Option 3 (chosen): Surface-discriminated.** Sign-in mismatch → S8; cached-token mismatch → silent sign-out. The surface identity carries the "have I been active" semantics for free, and both behaviors (ping-pong protection and customer agency at migration) are preserved.

**Option 4: Local "was previously active" boolean in Pane DB.** Pane persists a flag set on auto-claim or take-over confirmation; entitlement-check mismatch routes by flag value (true → silent sign-out; false → S8). **Rejected** as load-bearing on persisted state that doesn't pay for itself. The surface identity already discriminates the same way without the flag (sign-in calls happen only when not previously active; cached-token calls happen only after previously active). Adding a persisted boolean introduces failure modes (flag corruption, migration semantics, default-on-fresh-install ambiguity) the surface-discriminated approach avoids. If the future invariant in "Negative" above is ever violated (sign-in called with a cached token), this option becomes the cleaner fix; surface it then, not now.

**Option 5: Wrapper exposes a "displaced" signal explicitly.** `GET /v1/account/entitlement` returns a flag indicating whether the requesting `install_uuid` (sent in `X-Install-UUID` header per ADR-0027 §3) was previously the active machine. **Considered, deferred to post-launch.** Adds wire-contract scope on the wrapper (which would need to track `install_uuid` history, not just current value) for a property Pane can compute locally for free. If post-launch needs surface (e.g., support tooling wants "was this install displaced or never active"), the wrapper can add the signal additively without breaking ADR-0027 / ADR-0029. Not in v1 scope; filing here so a future contract amendment doesn't re-discover the option from scratch.

## References

- Spec section: `docs/spec/onboarding.md` §6.1.3 (Single-active-machine check) — amended in this session to land the surface-discriminated rule
- Spec section: `docs/spec/onboarding.md` §6.1.5 (Session storage layer) — `clear_auth_session()` contract that this ADR's silent-sign-out path invokes; the displaced-machine and S8 Cancel callers are already enumerated there (PR #1033)
- Related ADRs:
  - ADR-0024 (Local-Only Decryption) — silent sign-out preserves local data; cached-token mismatch is a UI/sign-out matter, not a data-access matter
  - ADR-0025 (Ecom Check Cadence — Launch + On-Focus) — defines the cached-token surface's two cadence call sites; this ADR pins their UI disposition. ADR-0025 §"Behavior on each check" already alludes to both behaviors and defers the discriminator; this ADR closes that defer
  - ADR-0026 (Install UUID for `active_machine_id` Matching) — supplies the ping-pong-friction rationale this ADR operationalizes; **not superseded**
  - ADR-0027 (Pane ↔ Ecom API Contract) — wire contract is unchanged; this ADR is Pane-side only
  - ADR-0029 (Entitlement Response Shape and Email-Check Privacy Posture) — entitlement snapshot fields consumed identically across both surfaces
- Origin: spec session 2026-05-10, resolving the §6.1.3 internal contradiction surfaced during pre-implementation review of #973
- Implementation target: Issue #973 (single-active-machine enforcement, Pane side); the surface-discriminated rule lands in the cached-token entitlement-check call site with an inline ADR-0035 citation per the "Negative" mitigation above
- Origin doc cross-references: `Pane_Onboarding_Redesign_Decisions__2026-05-06.md` § "State machine: take-over" and § "S8" (the silent-sign-out lock this ADR operationalizes)
