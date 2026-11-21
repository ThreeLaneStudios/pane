# ADR-0022: Notification State Is Per-Device, Not CRR-Synced

**Status:** Accepted
**Accepted:** 2026-07-07
**Date:** 2026-05-02
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The notification system needs to answer: when the same notification reaches multiple of the user's devices (Tier 1 server-emit, or any cross-device path), is read/dismiss state shared across devices, or is it per-device?

Three positions are coherent:

1. **Fully synced.** The `notifications` table joins the existing CRR tables. Read/dismiss state replicates via cr-sqlite. Reading on phone marks the desktop's row read.
2. **Per-device, no sync.** Each device's `notifications` table is independent. Reading on phone has no effect on desktop.
3. **Hybrid.** Notifications themselves replicate (so the same row appears on both devices); read/dismiss state is per-device.

The current implementation is closest to #2: `notifications` is not a CRR table; the cross-device path goes through Supabase REST polling, which inserts a *new* row in each device's table independently when the polling pass picks it up.

This question is entangled with the idempotency keys decided in ADR-0021: the keys are computed deterministically from the trigger conditions, so if a `task_due` trigger fires on both phone and desktop locally for the same task, both devices produce the same idem_key — but in their own independent DBs, so there's no collision and no resolution needed.

The architectural question to settle: which of #1/#2/#3 is the model going forward, and is `notifications` a CRR table or not?

### What if we didn't decide

Three failure modes:

- **The implementation drifts to "whatever's easiest per PR."** Read state ends up partially synced (because someone naturally reaches for cr-sqlite on the next PR), partially not (because the Supabase poll path doesn't update sibling devices). Users report inconsistent behavior; debugging is hard because the mental model is unclear.
- **Notifications becomes a CRR table by accident.** A future PR adds CRR for unrelated reasons (e.g., to make the table show up in cross-device backups), inheriting LWW semantics for read/dismiss without anyone explicitly choosing them. The dismiss latency goes from instant to "round-trip through Supabase relay" without anyone noticing until a user complains.
- **The cross-device emit story stays implicit.** `docs/spec/notifications.md` §19.8.2's "what types are local-emit vs cross-device-emit" mapping gets implemented per-type ad hoc, with the wrong defaults for some types.

## Decision

The `notifications` table is **per-device.** It is not a CRR table. Read state and dismiss state never replicate.

### Notification rows are local-DB-only

`notifications` stays out of `crsql_as_crr()`. Every device has its own table, its own row IDs, its own read/dismiss state. There is no `crsql_changes` traffic for notifications.

### Cross-device delivery uses content-identity, not row-identity

When a notification originates on device A and needs to reach device B, the path is the encrypted Supabase relay (already built). Each receiving device inserts its own row with its own UUID into its own table. The Supabase row's `read=true` mark prevents *re-delivery*, not cross-device read state.

The idempotency key on the encrypted payload guarantees a single device doesn't emit the same content twice if the trigger fires multiple times locally; it does not coordinate across devices.

### Type-by-type emit-origin specification

`docs/spec/notifications.md` §19.2's table is canonical. Each notification type is either *local-emit* (each device's local trigger fires independently — no cross-device coordination needed) or *cross-device-emit* (one origin, multi-device delivery via Supabase relay). The launch-scope types are all local-emit; cross-device-emit is reserved for Tier 1 server-side and future external integrations.

### INVARIANT recorded

A new invariant is added to `docs/INVARIANTS.md`:

> **Read and dismiss state for notifications is per-device.** The `notifications` table is not a CRR table. Sync of *content* uses the encrypted Supabase relay with idempotency keys; sync of *state* does not exist. (`docs/spec/notifications.md` §19.8, ADR-0022)

This is the mechanical guard: a future PR that converts `notifications` to CRR fails the invariant check (and the spec-vs-code audit if/when one runs against this section).

## Consequences

### Positive

- **Dismiss is always instant.** No round-trip, no user-visible latency, no failure mode where a dismiss appears to take effect and then re-appears after sync.
- **The cross-device story is simple to reason about.** "Each device gets its own copy when a cross-device path delivers; reading or dismissing affects only the device you do it on" matches user expectations from email clients, OS notifications, and every other multi-device notification surface.
- **Schema posture matches sync posture.** Notifications are an attention surface, not a content surface. The data model reflects that.
- **No CRR overhead.** `notifications` rows don't generate `crsql_changes` entries, which keeps the change feed focused on actual user content.
- **Failure isolation.** A bug in cr-sqlite cannot break the notification system. A bug in the notification system cannot break sync.

### Negative

- **Same notification can appear on two devices at the same time.** A user with phone and desktop both polling Supabase will see the same `task_due` notification on both. Mitigation: this is the user-expected behavior; "told once per device" is the email-client baseline.
- **Marking all read on one device doesn't clear the badge on the other.** Mitigation: same as above. Power users with strong "single source of truth" expectations can use the `mark_all_notifications_read` Tauri command which is per-device anyway.
- **The decision must be re-litigated if Pane ever wants per-account-not-per-device behavior** (e.g., a future shared-team workspace). Mitigation: re-litigate it then via a supersession ADR. Today's user model is single-user, so per-device is the right default.

### Risks

- **A future contributor doesn't read this ADR and adds CRR to `notifications` for an unrelated reason.** Mitigation: the INVARIANT in `docs/INVARIANTS.md` and a §11 audit catch this. As an additional belt-and-suspenders layer, `migrations.rs` could include a comment-block above the `notifications` schema definition explaining the decision; readers who jump straight to the code (the ADR-0019 failure mode) see the rationale at the point they're modifying the table.
- **Tier 1 hosted introduces a server-side emit path that benefits from cross-device read state.** This ADR doesn't preclude that — it specifies that the launch model is per-device, with cross-device-emit through the Supabase relay producing independent rows. A future Tier 1 design can add server-side emit semantics that Pane respects without changing this ADR; only adding new emit-origin types changes.

## Alternatives Considered

**Make `notifications` a CRR table; sync read/dismiss via LWW.** **Rejected.** The dismiss latency cost (round-trip through Supabase relay) is user-visible — clicks that appear instant become "click, ~30s lag, dismiss takes effect" worst-case. Email clients don't behave this way. OS notifications don't behave this way. The user mental model is per-device; matching it is correct.

**Hybrid: rows replicate via CRR, but read/dismiss is filtered by `device_id`.** **Rejected.** This is the worst of both worlds — sync overhead of CRR without the simpler semantic, plus a custom filter layer at every read point. The implementation surface is bigger than either pure choice.

**Push read/dismiss state up to the Supabase relay.** A user reads on phone → phone POSTs `read=true` for the relay row → desktop's next poll sees the marked-read state. **Rejected.** Same dismiss-latency problem as the CRR option, plus it doesn't generalize: most notification types in `docs/spec/notifications.md` §19.2 are local-emit and never go through Supabase. Designing the "every read syncs to Supabase" path means designing it for content that doesn't exist on Supabase, which is a strange shape.

**Defer the decision; let the implementation pick.** **Rejected.** This is the failure mode described in "What if we didn't decide" — the implementation drifts, the user-facing behavior gets inconsistent, and a postmortem has to retrofit the spec to whatever behavior shipped.

## References

- Companion ADR: ADR-0021 (Event-Driven Notification Triggers and Coordinator-Only AI Nudges). Filed alongside this ADR.
- Spec section: `docs/spec/notifications.md` §19.8.
- Related ADRs: ADR-0002 (CRDTs for Multi-Device Sync — frames what *is* CRR-synced; this ADR carves out what isn't); ADR-0004 (Zero-Knowledge Encryption — frames the Supabase relay's encryption boundary).
- Related INVARIANT: new entry in `docs/INVARIANTS.md` (Notification System): "Read and dismiss state for notifications is per-device. The `notifications` table is not a CRR table." (`docs/spec/notifications.md` §19.8, this ADR)
- Schema: `docs/spec/notifications.md` §19.9 (`notifications` table additions).
- Audit: `docs/audits/2026-05-01-notifications-audit.md`.
