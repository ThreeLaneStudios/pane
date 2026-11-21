# ADR-0026: Install UUID for `active_machine_id` Matching

**Status:** Accepted — Superseded in part by ADR-0070 (2026-11-09)
**Superseded by:** ADR-0070 for everything `active_machine_id`: Decisions 4–6 (send at sign-in, take-over confirmation, restore-preserves-UUID for take-over) are retired. Decisions 1–3 stand — `install_uuid` generation, lifetime and reset rules — because `backup.rs` writes it into backup metadata.
**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The onboarding redesign (#953) introduces single-active-machine enforcement at sign-in via an ecom-stored `active_machine_id` field (Dev Dispatch §X8, §State Machine: take-over). When a customer signs in, Pane sends an identifier for the current machine; ecom compares against the stored `active_machine_id` and either:

- claims as the active machine (no prior, or matches),
- prompts to take over (different machine claimed), or
- silently signs the displaced machine out on its next ecom check (per ADR-0025).

The choice of identifier governs the customer experience across reinstalls, hardware upgrades, OS reinstalls, and Time Machine restores. Three families of identifier are in scope:

- **Device fingerprint.** Composite of hardware identifiers (CPU type, MAC address ranges, screen resolution, OS version, locale, etc.). Survives reinstall on the same machine. Sensitive to hardware changes (RAM swap, SSD swap, motherboard replacement) — produces false-positive "new machine" results.
- **Install UUID.** Random UUID generated at first run, stored in Pane's local config. Stable across hardware changes; resets on full Pane reinstall (or any wipe of Pane's config directory).
- **OS machine ID.** macOS hardware UUID / Linux machine-id. Survives reinstall *and* hardware swaps within reason (motherboard replacement is the main edge case). Closest to "true hardware identity."

The dev dispatch (`Pane_Onboarding_Dev_Dispatch__2026-05-06.md`, ADR-T3) walked four customer scenarios — reinstall-to-clear-state, Time Machine restore, hardware upgrade, OS reinstall — across the three options and recommended Option B (install UUID). This ADR records the decision.

A complicating factor: the threat model for single-active-machine enforcement at launch is **customer fatigue** — legitimate customers accidentally creating multi-Pane installs and hitting silent data corruption per Dev Dispatch §X8 — not active adversaries. A customer trying to share their account across machines is mostly legit ("I have a desktop and a laptop and I want to use both"). The decision documented in §X8 is "v1 is single-machine; multi-Pane is Tier 1 hosted." The take-over UI is the customer-facing mechanism for moving the seat. Sneak-around prevention is secondary; gateway-side enforcement (sub-task #7) provides a separate trust boundary for OC paths, which are the actually-data-corrupting case.

## Decision

**`active_machine_id` matches against an install UUID generated at first run and stored in Pane's local config directory.**

Specifically:

1. **Generation.** On first launch (no existing UUID in config), Pane generates a v4 UUID via the OS's CSPRNG. Stores in `~/Library/Application Support/Pane/install-uuid.json` (macOS path; equivalent on other platforms when Pane ships there). The file contains exactly one record: `{"install_uuid": "<uuid>", "created_at": "<rfc3339>"}`. Pane never re-generates this UUID once written.

2. **Lifetime.** The UUID survives:
   - Pane app updates (config dir untouched on update)
   - macOS upgrades (config dir untouched)
   - Hardware upgrades (RAM, SSD, motherboard — config dir is on internal storage, separate from the parts being replaced; survives unless the storage device itself is replaced without restore)
   - Time Machine restore (config dir is restored as part of user data; the same UUID returns)

3. **Reset triggers.** The UUID is regenerated when:
   - Customer manually deletes the config directory
   - Pane is reinstalled with a "wipe data" choice (per X8 case 1)
   - The user's home directory is wiped or new-user setup begins
   - OS reinstall without restore (config dir lost)

4. **Send at sign-in.** On first sign-in (S1b password submit), Pane sends the install UUID to ecom alongside credentials. Ecom returns the take-over decision per Dev Dispatch §State Machine: take-over.

5. **Take-over confirmation overwrites ecom.** When the customer accepts the S8 take-over prompt, ecom updates `active_machine_id` to this machine's install UUID. The previous machine's install UUID becomes the displaced one and gets evicted on its next ecom check (per ADR-0025).

6. **Restored backup keeps the same UUID.** When a customer migrates via X8 case 2 (backup restore), the backup must include `install-uuid.json` so the restored install presents as the same machine. This is filed under Dev Dispatch sub-task #5 (backup file format verification); without it, the restored machine appears new and triggers the take-over prompt unnecessarily.

## Consequences

### Positive

- **Hardware upgrades preserve the install identity.** A customer swapping RAM, replacing an SSD via clone-and-swap, or upgrading their motherboard does not get treated as a new machine. This is the single biggest customer-friendliness win — the alternative (device fingerprint) produces false take-over prompts on legitimate upgrades, which is exactly the kind of friction that erodes trust.
- **Time Machine restore preserves identity.** The customer who hits "restore from Time Machine" after a hard drive failure lands on the same install UUID. The install retains its account binding without manual reconfiguration.
- **Reinstall = new machine.** The customer who reinstalls Pane (deliberately or via OS wipe) gets a new install UUID. Ecom sees a new machine claim. Take-over UX presents — one tap, customer is signed in. Acceptable friction given that the take-over UX is the legitimate path for moving seats anyway.
- **Tier 1 hosted forward-compat.** When Pane introduces multi-machine support in Tier 1, the install UUID is the natural identifier for "this Pane install" within a "this account has N installs" structure. The ID's local-only generation means migrating to multi-active doesn't require ID changes.
- **Implementation is simple.** UUIDv4 generation is one library call. Storage is one file. No platform-specific identifier APIs, no entitlement APIs, no fingerprinting heuristics.

### Negative

- **OS reinstall = new machine.** A customer doing a clean OS reinstall (rare; usually paired with Time Machine restore) loses the install UUID. They re-pair via take-over UX. Friction is acceptable for the rarity of the event; alternatives only fix this case at the cost of breaking the hardware-upgrade case (Option A) or being too aggressive on customer scenarios we haven't yet enumerated (Option C).
- **Reinstall-to-clear-state friction.** A customer who reinstalls Pane to "start fresh" still hits the take-over prompt because their previous install UUID is the active one. They tap Take Over and proceed — one extra step. Documented in dev dispatch as acceptable; surfacing here so onboarding-help copy (sub-task #16) can address it.
- **Sneak-around via reinstall.** A customer attempting to share their account across machines could, in principle, reinstall on each machine and use the take-over prompt to ping-pong. Each ping-pong evicts the previous machine. This ping-pong vector is unaddressed by any of A/B/C — different machines have distinct fingerprints, UUIDs, and OS IDs alike, so each take-over is "legitimate" by every identifier's lights. (Same-hardware reinstall-to-clear-state is a *different* threat, addressed by Options A and C but not by B.) Mitigation for the ping-pong case: gateway-side single-Pane pairing rejection (sub-task #7) holds the line for OC paths, which are the data-corruption-prone case. For BYOK paths, ping-ponging is annoying enough on its own (each switch logs the customer out of one machine) that the legit-customer-fatigue threat model dominates the adversarial threat model.

### Risks

- **`install-uuid.json` is in a user-writable location.** A motivated customer could edit the file, cloning their UUID to a second machine. Same threat-vector as the previous bullet; same mitigation. Mechanical hardening (encrypted config, OS-keychain storage of UUID) is possible but adds complexity for a marginal threat. Filed as future-work if launch evidence suggests abuse.
- **Backup-format dependency on UUID inclusion.** If sub-task #5 (backup format verification) finds the UUID is *not* included in backups, restored backups will fail the take-over check. Mitigation: sub-task #5 is launch-blocking; this ADR makes the dependency explicit so the sub-task gets the right scope.
- **First-launch race condition.** If Pane is launched twice in rapid succession (e.g., user double-clicks the dock icon), two processes might race to write `install-uuid.json`. Mitigation: implementation uses atomic file write (tempfile + rename) — same pattern used elsewhere (gateway tokens file, atomic note saves per §2.3A). Last-writer-wins in the rare race is fine; the UUID becomes whichever value won. Naming it so implementation doesn't skip the atomic write.

## Alternatives Considered

**Option A: Device fingerprint** — composite hardware identifier. **Rejected** because legitimate hardware upgrades (RAM, SSD, motherboard) flip the fingerprint and trigger false take-over prompts. The customer-friendliness loss exceeds the sneak-around prevention gain. Most customers do not hit hardware upgrades during a normal app lifetime — but the customers who do are exactly the customers who paid for premium hardware and expect their software to roll with the upgrade. Producing an "is this you?" prompt for someone who just upgraded their laptop's SSD is the wrong friction.

**Option C: OS machine ID** (macOS hardware UUID, `/etc/machine-id` on Linux). **Rejected** as too aggressive on the sneak-around threat for the cost. The OS machine ID survives Pane reinstall, OS reinstall (without backup), and most hardware changes — which is exactly the property a customer trying to legitimately migrate doesn't want. Migration scenarios (a customer setting up a fresh install on a new laptop after factory-reset of the old one) would be invisible to ecom and require manual support intervention. The benefit (catches sneak-around-via-reinstall) is real but small relative to gateway-side enforcement (sub-task #7), which protects the actually-data-corrupting threat (multi-Pane on OC paths) regardless of which identifier is chosen here.

**Option B': Composite of install UUID + lightweight hardware hint** — install UUID for primary identity, with an optional hardware fingerprint for "looks similar" tie-breaking on edge cases. **Considered, deferred.** Adds engineering complexity and a third state ("looks like the same machine but the UUID changed — do you want to claim?") that doesn't pay for itself in v1. If post-launch data shows install-UUID alone is producing too many take-over prompts in customer support tickets, this can be added on top — Option B is forward-compatible with future hardware-hint-as-tiebreaker logic.

## References

- Spec section: `docs/spec/onboarding.md` §6.1 (sign-in flow, take-over prompt)
- Spec section: `docs/spec/file-system.md` §2.3A (atomic write pattern, used for `install-uuid.json` first-write)
- Related ADRs:
  - ADR-0024 (Local-Only Decryption) — establishes that take-over is a UI/sign-out matter, not a data-access matter
  - ADR-0025 (Ecom Check Cadence) — defines when `active_machine_id` is checked; this ADR defines what it's matched against
- Related issues:
  - Dev Dispatch sub-task #5 (backup file format must include `install-uuid.json` — launch-blocking)
  - Dev Dispatch sub-task #6 (single-active-machine enforcement — ecom side)
  - Dev Dispatch sub-task #7 (gateway single-Pane pairing rejection — defense-in-depth for OC paths)
  - Dev Dispatch sub-task #16 (onboarding help copy: single-machine for v1 — sets expectation for take-over prompt)
- Origin: `Pane_Onboarding_Dev_Dispatch__2026-05-06.md` § ADR-T3 (locked recommendation: Option B)
