# ADR-0004: Zero-Knowledge Encryption for Sync

**Status:** Accepted  
**Date:** 2026-02-27  
**Author:** Matt Carano

## Context

Pane syncs data between devices via Supabase. The data includes personal notes, tasks, and AI conversations — highly sensitive content. Matt's philosophy (and Pane's product positioning) demands that no one — not Supabase, not us, not a breach attacker — can read user data.

## Decision

All synced data is encrypted client-side before leaving the device using a zero-knowledge pipeline:

1. **Key derivation:** User passphrase → Argon2id (memory-hard KDF) → master key
2. **Key expansion:** Master key → BLAKE3-HKDF → per-purpose encryption keys
3. **Encryption:** XChaCha20-Poly1305 (authenticated encryption, 192-bit nonce)
4. **Storage:** Encrypted blobs stored in Supabase. Server never sees plaintext.
5. **Key storage:** OS keychain (service `com.threelane.pane`, account `sync_master_key`), per #1498 / ADR-0044 Addendum A. **Updated 2026-06-17 (#1498):** the master key was originally stored at `~/Pane/.pane/sync_key.bin` (0600, never synced); it now lives in the OS keychain under ADR-0044's credential-storage contract. Migration from the legacy file is copy-then-clear with read-back confirmation, gated by a DB migration flag (`migration_flags` table) per ADR-0044 Decision 2. The derivation (Argon2id), expansion (BLAKE3-HKDF), encryption (XChaCha20-Poly1305), and zero-knowledge model are unchanged.

## Consequences

### Positive
- **True zero-knowledge** — Supabase stores ciphertext only. A database breach reveals nothing.
- **User owns their data** — No backdoor, no admin access, no "forgot password" recovery by us.
- **Aligns with values** — Privacy-first, local-first, anti-surveillance. This is the product story.
- **Strong primitives** — Argon2id (winner of Password Hashing Competition), XChaCha20 (misuse-resistant, large nonce space), BLAKE3 (fastest secure hash).

### Negative
- **No password recovery** — If user loses their passphrase and key file, synced data on other devices is unrecoverable. Local data is still accessible (not encrypted at rest on device).
- **No server-side search** — Can't query encrypted data. All search happens locally.
- **Key management burden** — User must not delete `sync_key.bin`. Backup guidance needed.
- **Performance overhead** — Encrypt/decrypt on every sync operation. Negligible for text, noticeable for large attachments.

### Risks
- Users will lose passphrases. We need clear UX around what "zero-knowledge" means in practice.
- If Argon2id or XChaCha20 are broken (extremely unlikely), we need a re-encryption migration path.

## Alternatives Considered

**Server-side encryption (at rest)** — Supabase encrypts storage. Rejected: protects against disk theft, not against Supabase employees, subpoenas, or breaches. Not zero-knowledge.

**No encryption** — Faster, simpler. Rejected: violates core product values. "Your data never leaves your device unencrypted" is a launch-day promise.

**Age encryption** — Simpler API. Rejected: not designed for streaming/incremental encryption of database changesets.

## References

- Sync encryption: architecture spec §5
- Key storage: OS keychain account `sync_master_key` (service `com.threelane.pane`) per #1498 / ADR-0044 Addendum A; legacy location `~/Pane/.pane/sync_key.bin` migrated via the copy-then-clear contract
- Supabase bucket: `pane-files`
- Related: ADR-0002 (CRDTs — what gets encrypted)
