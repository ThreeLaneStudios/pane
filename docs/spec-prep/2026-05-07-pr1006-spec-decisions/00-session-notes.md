# Session Notes — PR #1006 Spec Decisions (2026-05-07)

**Trigger:** PR #1006 (Track C, sub-task #972 — backup format) review by Reed. Two findings required spec-committee resolution before PR could merge.

**Inputs:**
- `2026-05-07-pr1006-backup-audit.md` (Reed's audit)
- ADR-0024 from prior session (`onboarding-spec-rewrite-2026-05-07/0024-local-only-decryption.md`)
- §6.1.4 from prior session (`onboarding-spec-rewrite-2026-05-07/02-spec-onboarding-section-6.md`)

**Locked decisions (do not relitigate):**
- T1/T2/T3 (from ADR-0024/0025/0026)
- Plaintext API keys in DB — out of scope, separate issue
- Pairing-code rate-limiting — out of scope, separate issue

---

## Decisions

### Decision 1 — Backup file encryption (ADR-0024 amendment)

**Resolution:** Reading 2 confirmed. The keychain export blob is **plaintext on disk**. "Encrypted" in the original phrasing referred to source-of-truth state (macOS keychain at rest), not the on-disk backup blob.

**Trust boundary pinned:** Backup file confidentiality at rest is the **user's** responsibility, not Pane's. Re-litigation is rebuttable but the bar is explicit (see amendment §"Re-opening conditions").

**Punted to post-launch:** Optional passphrase-based backup encryption as a Settings toggle. Tracked, not committed.

**Three load-bearing arguments:**
1. `.db` dominates the sensitivity surface (BYOK keys plaintext per separate issue) — encrypting only `_keychain.json` is theater.
2. Migration UX requires plaintext-equivalent-or-external-state; no v1-feasible decryption-key source preserves the migration use case.
3. macOS infra (FileVault, keychain at-rest, default backup directory not iCloud-synced) handles dominant cases; user-initiated exposures are documentable, not preventable.

**Output:** `2026-05-07-adr-0024-amendment-backup-encryption.md`

### Decision 2 — Restore error contract (§6.1.4 amendment)

**Resolution:** Three components, three behaviors per Reed's recommendation, with explicit step ordering and error-class enumeration.

| Component | Behavior |
|---|---|
| Schema `backup > current` | **Fatal.** Block before any state mutation; UI shows upgrade-Pane CTA. |
| Schema `backup < current` | **Confirmation.** UI prompts before mutation; user "Cancel" returns to pre-restore state. |
| Schema equal | Silent OK. |
| Install UUID write | **Fatal.** Returns `Err`; ADR-0026 take-over corrupts otherwise. |
| Keychain import | **Soft.** Warning surfaced in restore summary; recoverable via Settings cascade-UX. |
| `.pane/system/` extract | **Soft.** Re-syncs from gateway on next OC connection. |

**Load-bearing detail:** Schema check **must precede any state mutation.** Schema-too-new check after DB restore would corrupt local state on rollback.

**Output:** `2026-05-07-spec-onboarding-6-1-4-restore-error-contract.md`

---

## What unblocks

- **PR #1006** can merge once: (a) this amendment is applied, and (b) Finding 5 (parent dir creation — `install_uuid.rs::write_install_uuid` needs `fs::create_dir_all` before atomic write) is addressed. Findings 3/4/6 are non-blocking follow-ups per Reed.
- **Sub-task #971** (restore UI) can start; consumes the §6.1.4.1 contract directly.

## What's still open (out of scope for this session)

- Reed's Finding 3 — backup rotation doesn't delete sidecar files. Disk leak, low severity. Follow-up issue.
- Reed's Finding 4 — incomplete keychain discovery. Verify all credentials are backed up. Follow-up issue.
- Reed's Finding 5 — missing parent directory creation in `write_install_uuid()`. **Merge-blocking** for PR #1006 but implementation-only; no spec needed.
- Reed's Finding 6 — `PANE_TEST_ROOT` parallel-test safety. Test-only issue. Follow-up issue.
- Plaintext API keys in DB — separate security issue, separate ADR/spec session.
- Pairing-code wrong-attempt rate-limiting — separate side issue.
- Post-launch passphrase-based backup encryption — tracked, not committed.

---

## Outputs

In `/mnt/user-data/outputs/2026-05-07-pr1006-spec-decisions/`:

```
00-session-notes.md                                                (this file)
2026-05-07-adr-0024-amendment-backup-encryption.md                 (apply to docs/adr/0024-local-only-decryption.md)
2026-05-07-spec-onboarding-6-1-4-restore-error-contract.md         (apply to docs/spec/onboarding.md §6.1.4)
```

Each output file contains an "Implementation note for Brit" header with concrete str_replace targets and append/insert instructions.

## Cross-reference index

- ADR-0024 amendment ↔ §6.1.4 amendment: companion deliverables; both must land in the same PR (or as fast-follows). The §6.1.4 amendment refers to "ADR-0024 Amendment 2026-05-07"; the ADR amendment refers to onboarding.md §6.1.4 wording fixes.
- Both ↔ Reed's audit: cited as the trigger and source-of-truth for the findings.
- Both ↔ ADR-0026: install UUID load-bearing argument is referenced in both.
- §6.1.4 amendment ↔ §22C.11A.3 / §22C.11A.5: keychain soft-failure recovery path for cascade-UX.
- §6.1.4 amendment ↔ ADR-0026 §6: install UUID fatal rationale.

---

## Sequencing followed

1. Read Reed's audit + verify against current ADR-0024 / §6.1.4 wording in prior session output.
2. Decision 1 — encryption: walked the threat model + three options + .db-dominates argument; landed on Reading 2 + plaintext + trust boundary pinning.
3. Decision 2 — error contract: walked Reed's recommendations + step ordering analysis (schema check must precede state mutation); landed on three behaviors + error class enumeration for #971.
4. Drafted both files with concrete spec text changes (str_replace targets + new section content).
5. Drafted session notes (this file).

---

## Notes for Brit's PR

The two amendments are independent in content but related in trigger (same audit). Recommended sequencing:

- **Single PR** with both amendments + the §6.1.4 spec wording changes. This avoids leaving onboarding.md §6.1.4 ambiguous between amendments.
- Or **two PRs** if PR #1006 needs to merge faster than the §6.1.4 contract pin is implemented. In that case, the ADR-0024 amendment lands first (unblocks #1006); the §6.1.4 contract amendment lands second (unblocks #971).

Out-of-scope items above are NOT addressed in this session and should be filed as separate issues per Reed's audit recommendation. The audit already enumerates them as "Follow-Up Issues 1-5"; this session covered Issues 1 and 2 only.
