# ADR-0024 Amendment — Backup file encryption clarification

**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Triggered by:** PR #1006 review (Reed's audit, sub-task #972), Finding 1
**Supersedes:** None — this clarifies ambiguous phrasing in the original ADR-0024
**Status:** Accepted

> **Implementation note for Brit:** This is an amendment to the existing `docs/adr/0024-local-only-decryption.md`. Two surgical changes to existing prose (clarifying the ambiguous phrasing) plus one new section appended to the ADR. After this amendment lands, the spec text changes in §"Spec text changes" below also need to apply to `docs/spec/onboarding.md` §6.1.4 in the same PR — otherwise the spec wording remains ambiguous even though the ADR is resolved.
>
> Sequencing: PR #1006 is on hold pending this amendment. Once this amendment is merged, PR #1006 can land as-is (plaintext export is now spec-compliant); Finding 5 (parent dir creation) is the only remaining merge blocker per Reed's audit.

---

## Context

ADR-0024's original Consequences section states the backup file format must include "encrypted provider keys (or keychain export equivalent)" and onboarding.md §6.1.4 states the format must include "encrypted provider keys / keychain export and the install UUID". Reed's audit (PR #1006, Finding 1) confirmed this phrasing supports two readings:

- **Reading 1:** "Backup must include [encrypted (provider keys / keychain export)]" — the export blob itself must be encrypted on disk.
- **Reading 2:** "Backup must include [(encrypted provider keys) / keychain export]" — the provider keys are *already* encrypted at rest in macOS keychain; the export is decrypted-extraction for machine migration.

Stia implemented Reading 2 in PR #1006 (`src-tauri/src/backup.rs:207-220` writes plaintext JSON to `_keychain.json`). The audit flagged this as HIGH severity and deferred to spec committee for resolution.

We need to resolve the ambiguity, document the trust boundary, and stop re-litigating.

---

## Decision

**Reading 2 is correct: the `_keychain.json` export blob is plaintext on disk.** The "encrypted" adjective in the original phrasing refers to the source-of-truth state (macOS keychain encrypts at rest), not the on-disk backup blob.

This matches the shipped behavior in PR #1006 and the broader trust model established by ADR-0024 §1 ("Encryption keys live in the OS keychain") combined with the migration use case in onboarding.md §6.1.4 (X8 case 2).

**Backup file confidentiality at rest is the user's responsibility, not Pane's.** This is the trust boundary; everything below flows from it.

---

## Rationale

Three load-bearing points support Reading 2 over Reading 1.

**1. The `.db` file dominates the sensitivity surface.** A v1 backup contains:

| Component | Sensitivity | Currently |
|---|---|---|
| `.db` (SQLite) | High — BYOK API keys (plaintext per separate side issue), notes, chat history, ai_settings | Plaintext |
| `_keychain.json` | Medium — gateway_token (256-bit, 90-day), tls_cert_pin | Question on table |
| `_install-uuid.json` | Low — UUID alone is not sensitive | Plaintext |
| `_metadata.json` | Low — account_email, app version | Plaintext |
| `_system.tar.gz` | Medium — agent identity files (SOUL/MEMORY/IDENTITY) | Plaintext |

Encrypting `_keychain.json` while `.db` stays plaintext provides marginal protection — an attacker with access to the backup directory has the BYOK keys regardless. Asymmetric encryption of one component creates a false sense of security while leaving the dominant attack surface open. Either we encrypt the whole backup or we acknowledge the trust boundary explicitly.

(The plaintext-keys-in-DB issue is tracked separately and is out of scope for this amendment; what matters here is that fixing only the keychain blob does not meaningfully change the threat model.)

**2. Migration UX requires plaintext-equivalent or external state.** The whole point of the keychain export is that the destination machine doesn't have the source machine's keychain. The decryption key for the blob would need to come from somewhere:

- The source machine's keychain — defeats migration by definition.
- `sync_key.bin` (per ADR-0004) — lives in the source machine's keychain or filesystem; same problem at one remove.
- A user-entered passphrase — significant UX cost (forgotten passphrase = unrecoverable backup, worse than plaintext); requires a backup-creation step and a restore-time prompt that don't currently exist; not in scope for v1.
- Ecom-side stored secret — explicitly precluded by ADR-0024's "no ecom token dependency" stance.

There is no v1-feasible decryption-key source that preserves the migration use case. Reading 1 is technically achievable only with the passphrase option, which we are explicitly punting to post-launch.

**3. macOS handles the dominant cases via existing infrastructure.** FileVault is on by default for new Macs since macOS 10.10. Time Machine inherits FileVault encryption when the destination drive is itself encrypted. The default backup directory `~/Library/Application Support/Pane/Backups/` is not synced to iCloud Drive by default. The cases where backup files end up exposed are user-initiated (manual copy to cloud storage, third-party tools sweeping `~/Library`, emailing a backup as a bug-report attachment) — these are user choices that we can document but not prevent.

---

## Threat model — pinned

Pinning this explicitly so we stop re-litigating.

### In scope (Pane's responsibility)

- **macOS keychain at-rest encryption** for live credentials. Source-of-truth is encrypted by macOS; this ADR doesn't add or subtract from that.
- **Atomic write semantics** for backup files (file-system.md §2.3A). Backups are not corrupted by mid-write crashes.
- **Permissions on backup files** when Pane creates them. Default permissions match Pane's data directory (0700 on the parent directory; files inherit user-only access).
- **Documentation** that backup files contain extracted credentials — explicit security guidance to users.

### Out of scope (user's responsibility)

- **Where the user stores their backup file.** If the user copies a backup to iCloud Drive, Dropbox, Google Drive, or attaches it to an email, that is a user choice with user consequences. Pane does not prevent or detect this.
- **What happens to the backup file under Time Machine / third-party backup tools.** If FileVault is off and Time Machine writes to an unencrypted external drive, the backup file lands on that drive in clear. User-controlled, FileVault is the correct mitigation, not Pane-side blob encryption.
- **What happens if the user's machine is compromised.** Local-attacker-with-filesystem-access is outside the ADR-0024 trust model (and is the same threat model the live `.db` file faces).

### Re-opening conditions

This decision is rebuttable, but the bar is explicit. Reasons that would justify re-opening:

- A common Pane user-flow is identified that ends with a backup file in cloud storage by default (not by user choice).
- A downstream issue is tracked to Reading 2 specifically (e.g., a customer's backup leaks via a documented channel that wasn't on the in-scope list above).
- Tier 1 launches with passphrase-based encryption as a feature, at which point this ADR's punt becomes the previous-state.

Reasons that do **not** justify re-opening (these are recurring re-litigation patterns):

- "But what if the user copies it to iCloud?" — addressed in Out of scope above; user-responsibility.
- "But Reading 1 sounds more secure." — addressed in Rationale point 1; encrypting one component while `.db` is plaintext is theater.
- "But other backup tools encrypt their backups." — they typically do so with passphrases (which we are punting) or with platform keychain wrapping (which is circular for the migration case).

---

## Storage guidance for users

The post-launch documentation should include:

> **Pane backups contain credentials.** Your backup file (`.db` plus its `_keychain.json`, `_install-uuid.json`, `_metadata.json`, and `_system.tar.gz` companions) contains your provider API keys, gateway authentication tokens, and agent identity files. Treat the backup file like a password: store it on an encrypted drive (FileVault is on by default for new Macs), do not post it in cloud-shared folders, and do not attach it to bug reports without redacting credentials. If your backup file ends up in cloud storage, rotate your provider API keys and re-pair your OpenClaw gateway.

This guidance lives in:
- The backup-creation Settings UI when v1 ships (sub-task to file).
- The export-backup confirmation dialog (warning text before file is written).
- Pane's user-facing documentation under "Backups."

---

## What's not decided (post-launch options)

This amendment punts the following to post-launch tracking:

**Optional passphrase-based backup encryption.** Add a Settings toggle "Encrypt my backups with a passphrase." When enabled, backup creation prompts for a passphrase; the passphrase derives a key (Argon2id or scrypt) that encrypts the `_keychain.json` and `.db` files. Restore prompts for the same passphrase. Forgotten-passphrase = unrecoverable backup (this is the standard tradeoff and acceptable for an opt-in feature).

This is filed as a roadmap item, not a launch blocker. The v1 plaintext default + documentation is the v1 stance.

**Whole-backup encryption with sync_key.bin or a keychain-wrapped key.** Not pursued because the migration use case (the whole reason backup exists) breaks under either scheme. If the encryption key is on the source machine only, the destination machine cannot decrypt; if the encryption key travels with the backup, the encryption is theater.

---

## Spec text changes

Three concrete edits, applied in the same PR as this amendment.

### Change 1 — ADR-0024 Consequences (existing line)

**File:** `docs/adr/0024-local-only-decryption.md`

**Find:**

> - **Backup file format must include encrypted provider keys (or keychain export equivalent) for X8 case 2 (machine migration) to work.** Per Dev Dispatch sub-task #5, this is filed for verification — backup currently includes DB + settings, may not include keychain material. If it doesn't, restoring on a new machine would land the customer with database rows whose keys are unreadable. Sub-task #5 is launch-blocking; this ADR makes the dependency explicit.

**Replace with:**

> - **Backup file format must include a keychain export (plaintext JSON, see Amendment 2026-05-07) and the install UUID for X8 case 2 (machine migration) to work.** "Plaintext" here is intentional and trust-boundary-pinned; backup file confidentiality at rest is the user's responsibility per the amendment. Sub-task #5 / PR #1006 (#972) implemented this. The amendment section below pins the threat model explicitly.

### Change 2 — ADR-0024 Risks/Limitations (existing line)

**File:** `docs/adr/0024-local-only-decryption.md`

**Find:**

> - **Keychain corruption / OS reinstall scenario.** If a customer's OS keychain is wiped (full OS reinstall without Time Machine), the encryption keys are lost and the local DB becomes unreadable. This is orthogonal to ADR-T1 — the same loss happens with token-required decryption if the token cache is in keychain. Mitigation: backup file format (sub-task #5) must include keychain export. Frame as "if you lose your machine and have no backup, you lose your data" — same as any local-first product.

**Replace with:**

> - **Keychain corruption / OS reinstall scenario.** If a customer's OS keychain is wiped (full OS reinstall without Time Machine), the encryption keys are lost and the local DB becomes unreadable. This is orthogonal to ADR-T1 — the same loss happens with token-required decryption if the token cache is in keychain. Mitigation: backup file format (sub-task #5 / PR #1006) includes a plaintext keychain export per Amendment 2026-05-07; restoring on a new machine reseeds the keychain from this export. Frame as "if you lose your machine and have no backup, you lose your data" — same as any local-first product.

### Change 3 — onboarding.md §6.1.4 (existing line)

**File:** `docs/spec/onboarding.md` §6.1.4 (in the Restore bullet of the Backup-restore offer block)

**Find:**

> - **Restore** → file picker. Selected backup is unpacked into local DB + settings + `.pane/system/`. Skips the picker entirely; lands directly in Home with restored data. Backup format must include encrypted provider keys / keychain export and the install UUID — sub-task #5 verifies the backup format covers these (launch-blocking; ADR-0024 and ADR-0026 both depend on it).

**Replace with:**

> - **Restore** → file picker. Selected backup is unpacked into local DB + settings + `.pane/system/`. Skips the picker entirely; lands directly in Home with restored data. Backup format includes a plaintext keychain export (`_keychain.json`) and the install UUID (`_install-uuid.json`) per ADR-0024 Amendment 2026-05-07. Sub-task #5 / PR #1006 (#972) implemented this. Backup file confidentiality at rest is the user's responsibility per the amendment's trust boundary.

### Change 4 — Append new section to ADR-0024

**File:** `docs/adr/0024-local-only-decryption.md`, appended after the existing Risks/Limitations section and before the References section.

**New section:**

```markdown
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
```

---

## References

- Reed's audit: `docs/audits/2026-05-07-pr1006-backup-audit.md` Finding 1
- Pane source: `src-tauri/src/backup.rs:207-220`, `src-tauri/src/keychain.rs:29`
- Original ADR: `docs/adr/0024-local-only-decryption.md`
- Spec: `docs/spec/onboarding.md` §6.1.4 (X8 case 2 — backup-restore offer)
- Related: ADR-0026 (install UUID — companion data in backup); ADR-0004 (sync_key.bin — considered and rejected as decryption-key source for migration use case)
- Out of scope tracking: plaintext API keys in DB (separate issue), pairing-code rate-limiting (separate issue), Reed's PR #1006 Findings 3/4/5/6 (separate follow-up issues per the audit's recommendation)
