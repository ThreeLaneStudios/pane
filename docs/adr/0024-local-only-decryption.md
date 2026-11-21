# ADR-0024: Local-Only Decryption for Pane User Data

**Status:** Accepted
**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The onboarding redesign (#953) introduces ecom-backed account/entitlement management at sign-in: subscription status, single-active-machine enforcement (`active_machine_id`), and lapse handling. Pane already encrypts API keys, gateway tokens, and TLS pins in the OS keychain (per §22C.5.1, §6.5). The redesign's state machine raises a question the current spec does not answer explicitly:

**Does Pane require an ecom-issued token to decrypt locally-stored user data on app launch, or can decryption complete with only OS-keychain credentials?**

The answer is load-bearing for several behaviors:

- **Offline launch viability.** If decryption requires an ecom roundtrip, every app launch hard-blocks when ecom is unreachable. With local-only decryption, the app can launch and read existing data offline; ecom is consulted for entitlement gating only.
- **Lapse hard-block UX (S7).** With local-only decryption, the lapse hard-block is a UI overlay — features lock, but underlying data remains decryptable. Markdown files on disk continue to be openable in any text editor. With token-required decryption, lapse means the data goes opaque even on the customer's own machine.
- **First-sign-in vs returning-launch distinction.** First sign-in requires ecom (to verify the account exists). Returning launches with cached sign-in are the case this decision governs.
- **Tier 1 hosted forward-compat.** Tier 1 hosted will introduce server-side encryption for synced content per ADR-0004 (zero-knowledge). The local-only-decryption decision for Tier 0 (single machine, no cloud sync) does not preclude Tier 1's separate encryption model — they are different systems with different trust roots.
- **Brand promise — "your notes, your machine, your rules."** A token-gated decryption model retains the customer's data only at our service's pleasure. Local-only decryption matches the promise as stated.

The dev dispatch (`Pane_Onboarding_Dev_Dispatch__2026-05-06.md`) raised this as ADR-T1 with three options and a recommendation. This ADR records the decision.

## Decision

**Decryption of locally-stored Pane user data uses OS-keychain credentials only. No ecom-issued token is required to decrypt data on a paired install.**

Specifically:

1. **Encryption keys live in the OS keychain.** macOS Keychain at launch; same pattern as the existing `gateway_token` and `tls_cert_pin` storage. They are never transmitted to ecom and never derived from ecom-supplied material. (BYOK provider API keys at launch are stored in the local DB rather than the keychain — encrypted at the DB layer per this ADR's local-only decryption stance, not as a keychain entry. Distinct from the keychain-stored credentials above; tracked as a separate hardening issue.)

2. **First sign-in still requires ecom.** The X6 case 1 hard-block at first sign-in (ecom unreachable) is preserved — without ecom, account existence cannot be verified and the account-↔-entitlement binding cannot be established. This ADR governs returning launches, not first sign-in.

3. **Returning launches do not require ecom for decryption.** Per the launch state machine (Dev Dispatch deliverable 3), if local DB exists and cached sign-in is valid, the app can decrypt and read existing data even when ecom is unreachable. The ecom check (subscription, `active_machine_id`) runs alongside and gates entitlement-dependent features, not data access.

4. **Lapse hard-block (S7) is a UI overlay over decryptable data.** When ecom flips `subscription_status` to `lapsed` and Pane sees it on the next ecom check (per ADR-0025), the app surfaces the renewal screen and blocks chat / memory extraction / OC pairing. Local notes, projects, and `.pane/system/{agent}/*.md` files remain decryptable; they remain openable directly via the file system through any text editor.

5. **`onboarding_completed_at` and provider state remain readable post-lapse.** These fields drive launch routing and must be readable in order to display the lapse screen at all. They sit inside the encrypted store; local-only decryption allows them to be read without ecom.

6. **The decision is bounded to local data on Tier 0 single-machine installs.** Tier 1 hosted introduces server-side encryption keyed off account credentials (see ADR-0004); that is a separate trust root and a separate ADR's territory. This ADR governs only the local single-machine case Pane ships at launch.

## Consequences

### Positive

- **Offline launch on a paired install is possible.** Customer can read their notes on a flight, in a coffee-shop dead zone, on a poor connection. Matches the local-first commitment in ADR-0007.
- **Lapse UX is honest about data ownership.** "Renew to use Pane" doesn't mean "Renew or your notes go opaque." Files remain accessible. Standard SaaS pattern (Notion, Obsidian Sync, Bear) — referenced in dev dispatch working notes.
- **Simplest implementation.** No token-distribution layer between ecom and the local decryption path. The keychain is already the storage layer for sensitive material; this ADR records that we do not add a second layer above it.
- **Forward-compatible with Tier 1.** A future hosted Pane service can introduce server-side encryption keyed off account credentials without retrofitting Tier 0; the two systems' trust roots are independent. Migration is a re-encrypt step at Tier 1 onboarding, not a redesign of Tier 0.

### Negative

- **Subscription enforcement is UI-layer only on Tier 0.** A customer with technical sophistication and a lapsed subscription could continue to read MD files directly. This is intentional — see brand promise above. Mitigation: the ecom-side check at every focus event (ADR-0025) keeps the UI correctly locked; the data remains the customer's regardless.
- **Backup file format must include a keychain export (plaintext JSON, see Amendment 2026-05-07) and the install UUID for X8 case 2 (machine migration) to work.** "Plaintext" here is intentional and trust-boundary-pinned; backup file confidentiality at rest is the user's responsibility per the amendment. Sub-task #5 / PR #1006 (#972) implemented this. The amendment section below pins the threat model explicitly.
- **First-sign-in flow remains ecom-coupled.** The X6 case 1 hard-block is preserved. A first-launch customer with no ecom connectivity cannot complete sign-in and proceed to the picker. Correct (per dev dispatch: "duplicate accounts are worse than launch-day frustration") but worth naming as a consequence of *not* generalizing the local-only stance to first-sign-in.

### Risks

- **Customer service edge case: "I lapsed but I can't sign in to renew."** If a customer's machine is offline at the moment they try to renew, they're locked out of Pane entirely. Renewal happens at the ecom site (browser), not in-app, so this is a transient connectivity issue rather than a Pane-architecture issue. Mitigation: lapse hard-block still allows the customer to dismiss to read-only access of files via Finder; the Pane app stays locked, the data does not.
- **Keychain corruption / OS reinstall scenario.** If a customer's OS keychain is wiped (full OS reinstall without Time Machine), the encryption keys are lost and the local DB becomes unreadable. This is orthogonal to ADR-T1 — the same loss happens with token-required decryption if the token cache is in keychain. Mitigation: backup file format (sub-task #5 / PR #1006) includes a plaintext keychain export per Amendment 2026-05-07; restoring on a new machine reseeds the keychain from this export. Frame as "if you lose your machine and have no backup, you lose your data" — same as any local-first product.
- **Lapse-bypass via reinstall.** A customer could attempt to bypass lapse by reinstalling Pane (clearing the cached `subscription_status`) and signing in again. The first-sign-in still hits ecom, which still returns `lapsed` — so the bypass doesn't work. The dependency on ecom for first-sign-in is what makes this ADR's local-only stance compatible with subscription enforcement.

## Alternatives Considered

**Option B: Decryption requires ecom-issued token.** Pane fetches a decryption token from ecom on every launch; without it, local DB cannot be opened. **Rejected** because it violates the brand promise (notes lock when our service is unreachable), introduces a token-distribution layer that doesn't pay for itself in entitlement enforcement (ecom already enforces via subscription state), and creates a worst-case failure mode where ecom outage = global Pane lockout for every paying customer simultaneously. The single-vendor-outage exposure for what amounts to UI locking is the wrong trade.

**Option C: Hybrid — local token cached for an offline grace window (e.g., 7 days).** Pane caches a token from ecom that decrypts data for N days; after expiry, a fresh ecom call is required. **Rejected** as over-engineered for v1. The grace window introduces a third state (between "fresh" and "stale") with its own UX questions ("you have 2 days left to reconnect"), test surface, and rotation logic. Option A delivers the user-visible benefits (offline launch, lapse-doesn't-lock-files) without the rotation infrastructure. If post-launch threat-model evolution justifies grace-window-based gating, it can be added — but it is never going to be cheaper to add it after observing real customer usage than to build it speculatively now.

## Amendment — Backup file encryption (2026-05-07)

This section resolves an ambiguity in the original Consequences phrasing
("encrypted provider keys / keychain export") flagged by Reed's PR #1006
audit (Finding 1).

**Resolution:** The keychain export blob is plaintext on disk. "Encrypted"
in the original phrasing refers to the source-of-truth state (macOS
keychain at rest), not the on-disk backup blob.

**Trust boundary:** Backup file confidentiality at rest is the user's
responsibility, not Pane's. Pane is responsible for atomic-write
semantics, secure default permissions, and documentation. The user is
responsible for where they store their backup files (FileVault, encrypted
external drives, not in cloud-synced folders).

**Threat model:** Pinned in the amendment text (see "Threat model —
pinned"). Re-opening this decision requires meeting the rebuttable-burden
conditions explicitly listed there.

**Post-launch path:** Optional passphrase-based encryption is tracked as a
post-launch settings toggle. v1 ships plaintext-by-default with explicit
storage guidance.

Full amendment text and rationale: see PR (this PR), session deliverables
`2026-05-07-pr1006-spec-decisions/2026-05-07-adr-0024-amendment-backup-encryption.md`.

## References

- Spec section: `docs/spec/onboarding.md` §6 (rewritten in this session for #953)
- Spec section: `docs/spec/sync.md` §22C.5.1 (existing keychain storage pattern: `gateway_token`, `tls_cert_pin`)
- Related ADRs:
  - ADR-0007 (Local-First Architecture) — this ADR is a corollary
  - ADR-0004 (Zero-Knowledge Encryption for Sync) — Tier 1 hosted's encryption model, not in scope here
  - ADR-0025 (Ecom Check Cadence Post-Launch) — the entitlement-gating layer that this ADR's data-access stance is independent of
  - ADR-0026 (Install UUID for `active_machine_id` Matching) — the take-over enforcement mechanism that runs alongside this ADR's decryption stance
- Related issues: #953 (onboarding redesign umbrella), Dev Dispatch sub-task #5 (backup file format verification — launch-blocking), sub-task #6 (single-active-machine enforcement)
- Origin: `Pane_Onboarding_Dev_Dispatch__2026-05-06.md` § ADR-T1 (locked recommendation: Option A)
