# ADR-0025: Ecom Check Cadence — Launch + On-Focus

**Status:** Retired — Superseded by ADR-0070 (2026-11-09)
**Superseded by:** ADR-0070. There is no entitlement check to schedule: Pane opens to Home with no sign-in, and `auth_check_entitlement`, the launch + on-focus call sites and the post-take-over call site (ADR-0036) are removed with the ecom wrapper.
**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The onboarding redesign (#953) relies on an ecom backend for two ongoing checks during the customer's session:

1. **Subscription status** — `subscription_status` ∈ {active, lapsed, cancelled}. Drives the lapse hard-block (S7) and the future grace-period banner (Dev Dispatch sub-task #15).
2. **Single-active-machine** — `active_machine_id` matches the install UUID for this machine (per ADR-0026). Drives the take-over prompt (S8). When two machines fight, the most-recent-claimer wins; the displaced machine sees a mismatch and signs out.

Both checks need a cadence policy. Run them too rarely and customers experience stale entitlement state — they keep using the app for a full session after a lapse, or they don't notice their account was taken over from another machine. Run them too often and we burn battery, leak per-session telemetry to ecom (privacy noise), and add latency at every check.

The dev dispatch (`Pane_Onboarding_Dev_Dispatch__2026-05-06.md`) raised this as ADR-T2 with three options and a recommendation. This ADR records the decision.

Constraints worth naming:

- **Pane is a desktop app.** The relevant interaction model is "launch the app, work for hours, focus away to other apps, focus back." Customers don't reload Pane the way they reload a web tab.
- **Pane is local-first.** Per ADR-0024, local data is fully usable without ecom; ecom checks only gate entitlement-dependent features (chat, OC pairing, memory extraction). Stale entitlement state means features remain enabled longer than they should — not data loss.
- **The displaced-machine path (take-over) is asymmetric.** The *other* machine has already updated `active_machine_id`. The displaced machine just needs to *notice*. The cadence is what determines how long the displaced machine continues operating before it's evicted.

## Decision

**Pane checks ecom subscription status and `active_machine_id` at exactly two moments:**

1. **Every app launch.** After local DB is opened and sign-in token is verified, Pane runs the ecom account-status fetch (returns `subscription_status`, `active_machine_id`, `account_first_name`). Result drives launch routing per the Dev Dispatch state machine (S1 / S7 / S8 / continue).

2. **On window focus after backgrounding.** When the Pane application window receives the system focus event after being backgrounded (the user was on another app and switched back to Pane), Pane runs the same account-status fetch. If status changes (lapse appeared; `active_machine_id` flipped to a different machine), Pane transitions UX state to S7 or S8 immediately on this check.

No other periodic polling. No on-message-send check. No on-tab-switch within Pane.

**Behavior on each check:**

- Cached `subscription_status` and `active_machine_id` are updated in memory (and persisted to DB at the implementation layer's discretion).
- If `subscription_status` flips to `lapsed`, the lapse banner / hard-block surfaces on this check.
- If `active_machine_id` does not match this machine's install UUID (per ADR-0026), Pane surfaces S8 take-over UX or — if take-over has already been confirmed elsewhere — silent sign-out + return to S1.
- If ecom is unreachable on a focus check, the previous cached state remains in effect. No degradation. The next focus event retries.

## Consequences

### Positive

- **Lapse propagates within minutes for active users.** A customer who lapses while away from the keyboard sees the renewal banner the next time they focus Pane — typically ≤1 minute for an active session. No need to wait for the customer to relaunch.
- **Take-over propagates within minutes for active users.** A customer signing in on a new laptop displaces the old one. The old machine notices on its next focus event (or next launch if it stayed open in the background indefinitely).
- **No periodic background telemetry.** Pane does not phone home every N minutes regardless of whether the customer is using the app. Privacy story: ecom only sees calls when the customer is actively engaging with Pane. Battery story: zero timer-driven cost; the check is event-driven only.
- **Implementation is simple.** Two call sites (launch path; window focus handler). No timer infrastructure, no jitter, no exponential backoff. The platform's window-focus event is the trigger.
- **Cached state remains correct UX boundary on transient outage.** Ecom blip = focus check fails = previous cached `active` continues until the next focus event succeeds. No false lapse-banners triggered by ecom flapping.

### Negative

- **Backgrounded Pane with no focus event for hours doesn't update.** A customer who launches Pane in the morning, focuses to other apps for the whole day, and comes back at 6 PM gets one ecom check at 6 PM. If lapse happened at noon, the UI was technically out of date for 6 hours. Acceptable: this only matters for entitlement-dependent features the customer wasn't actively using anyway. If they had tried to send a chat at noon, the cached state would have allowed it; the next focus check catches up.
- **Ecom-unreachable on first launch is still hard-block.** This ADR doesn't soften the X6 case 1 first-sign-in hard block — that's an entitlement question, not a cadence question. Worth restating: cadence applies after first-sign-in succeeded.
- **Take-over from a malicious second-machine is bounded by the displaced machine's next focus event, not by minutes.** If the displaced machine is unattended (laptop left open, screen locked) and never receives a focus event, the take-over may not propagate for hours. Mitigation: gateway-side single-Pane pairing rejection (Dev Dispatch sub-task #7) provides a separate trust boundary for OC paths — the actual data-corruption-prone case. For non-OC paths, the worst case is "displaced machine continues operating for the duration of an unattended session" — the data on disk is still theirs (per ADR-0024); only the UI continues to think it's still signed in.

### Risks

- **Window-focus events don't fire reliably across all macOS versions / OS configurations.** Some accessibility settings, fullscreen modes, or virtual-desktop arrangements alter focus event delivery. Mitigation: implementation should cross-check by also running the ecom check on chat-send (a "user-initiated" trigger besides focus). Filed as a follow-up to be added if focus-only proves unreliable in dogfood — naming it here so the implementation team has the option without re-litigating the ADR.
- **Ecom rate limits if a customer rapidly cycles focus.** A customer alt-tabbing between apps repeatedly could fire many focus events per minute. Mitigation: implementation debounces focus checks to one per minute — first focus event in a window triggers the call; subsequent focus events within the debounce window use cached state. Filed as implementation discipline; the ADR locks the cadence pattern, not the debounce mechanism.
- **Privacy concern: ecom learns when the customer focuses Pane.** This is a low-grade telemetry signal that wouldn't exist with launch-only cadence. Mitigation: the ecom call is account-status only — it does not include usage data, content, or session detail. The fact that the customer is using Pane is already implicit in their having an active account; the additional information is "they switched to Pane at time T," which is benign.

## Alternatives Considered

**Option A: Every launch only.** Pane runs the ecom check at app launch and never again during the session. **Rejected** because lapse and take-over propagation depend on the customer relaunching the app. A long-running session (Pane stays open across multiple workdays, common on macOS) would never refresh entitlement state. The customer experience: "I lapsed three days ago and Pane has happily been working the whole time." This produces a deeply confusing customer-service touch ("how am I supposed to renew if your app pretends I'm fine?") and correctness drift for take-over scenarios.

**Option C: Periodic poll (every 15 minutes during session).** Pane runs the ecom check on a timer regardless of customer activity. **Rejected** because: it adds a network call every 15 min for the entire active session — leaking activity windows to ecom; it adds battery cost for the timer (small but nonzero on laptops); and it produces nothing meaningful that on-focus doesn't, since the propagation event the customer cares about (lapse banner appears, take-over detected) only matters when they're actually looking at the app. On-focus is the same UX result with less mechanical baggage.

**Option B': On-focus + on-chat-send.** Add a third trigger to belt-and-suspenders the focus event reliability concern. **Considered, deferred to implementation.** The ADR locks two triggers (launch, focus); the implementation can add a third (chat-send) without re-opening the ADR if focus-only proves insufficient in dogfood. If added, the chat-send check must be cached / debounced so a chat session doesn't fire one ecom call per message.

## References

- Spec section: `docs/spec/onboarding.md` §6.1 (sign-in, ecom check at launch)
- Spec section: `docs/spec/sync.md` §22C (gateway operations independent of ecom; OC pairing has its own trust boundary)
- Related ADRs:
  - ADR-0024 (Local-Only Decryption) — establishes that ecom is for entitlement only, not data access; this ADR specifies *when* entitlement is checked
  - ADR-0026 (Install UUID for `active_machine_id` Matching) — defines what `active_machine_id` is matched against; this ADR specifies *when* it's matched
- Related issues: Dev Dispatch sub-task #6 (single-active-machine enforcement — ecom side), sub-task #7 (gateway single-Pane pairing rejection — defense-in-depth for OC paths)
- Origin: `Pane_Onboarding_Dev_Dispatch__2026-05-06.md` § ADR-T2 (locked recommendation: Option B)
