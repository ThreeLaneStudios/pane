# Spec Amendment — onboarding.md §6.1.4 restore error contract

**Date:** 2026-05-07
**Author:** Matt Carano, Claude (spec session)
**Triggered by:** PR #1006 review (Reed's audit, sub-task #972), Finding 2
**Status:** Accepted — pins behavior before sub-task #971 (restore UI) starts

> **Implementation note for Brit:** This adds a new sub-section §6.1.4.1 to `docs/spec/onboarding.md`, immediately after the existing §6.1.4 (Backup restore offer) and before §6.2 (Provider picker). The existing §6.1.4 prose stays unchanged; this is purely additive.
>
> Sequencing: PR #1006 currently swallows errors silently per Reed's Finding 2. This spec amendment pins the behavior so PR #1006 can land the error-handling fix in the same PR as the encryption clarification (or as a fast-follow). Sub-task #971 (restore UI) is on hold until this amendment is merged — the UI consumes this contract.

---

## Context

Reed's PR #1006 audit (Finding 2) confirmed `restore_backup()` swallows errors after the DB-level restore succeeds:

- Install UUID restore failure → `eprintln!` + continue, returns `Ok(())`
- Keychain restore failure → `eprintln!` + continue, returns `Ok(())`
- Schema version mismatch → `eprintln!` + continue, returns `Ok(())`

Silent partial restore creates ghost state: the user thinks migration succeeded, then discovers later that take-over fires incorrectly (UUID wasn't restored), keys are missing (keychain wasn't restored), or data behaves oddly (schema was incompatible). All three cases are recoverable, but only if the user is told what went wrong at restore time.

This sub-section pins the per-component error contract so #1006 can implement it correctly and #971 (restore UI) can consume it correctly.

---

## Decision

Three components, three behaviors:

| Component | Failure mode | Behavior |
|---|---|---|
| Schema version check | `backup.schema > current` | **Fatal — block before any state mutation.** Returns `Err`. UI shows upgrade-Pane message. |
| Schema version check | `backup.schema < current` | **Soft — confirmation required.** UI shows warning with "Restore anyway" / "Cancel" buttons. Forward migrations will run on next launch. |
| Install UUID restore | Write failure | **Fatal — return `Err`.** ADR-0026 take-over mechanics break if UUID is missing; partial restore would leave the customer with active_machine_id mismatch they can't resolve. |
| Keychain restore | Import failure | **Soft — log warning, return `Ok`.** Recoverable via Settings cascade-UX (§22C.11A.3); transient keychain locks are common and shouldn't fail an otherwise-successful restore. |
| `.pane/system/` extraction | Tar extraction failure | **Soft — log warning, return `Ok`.** Identity files re-sync from gateway on next OC connection; restoring without them is degraded but not fatal. |

The "fatal" cases halt restore and surface a recoverable error to the user. The "soft" cases continue restore and surface a warning the user can address later.

---

## Restore step ordering (load-bearing)

The schema check **must precede any state mutation.** A schema-too-new check that runs after DB restore would leave the local DB in a corrupted state after rollback, which is worse than not restoring at all.

Required order:

1. **Read metadata** from backup (`_metadata.json`). No state mutation; pure read.
2. **Schema compatibility check:**
   - `backup.schema > current` → return `Err(SchemaTooNew)` immediately. No state touched.
   - `backup.schema < current` → surface confirmation to UI (#971); on user "Cancel," return `Err(UserCancelled)`. No state touched on cancel.
   - `backup.schema == current` → proceed silently.
3. **Restore DB** (existing behavior; succeeds or returns `Err`).
4. **Restore install UUID** — if write fails, return `Err(InstallUuidRestoreFailed)`. State at this point: DB is restored but UUID is the old/missing one; this is the last step where atomicity is achievable. The error message must indicate that the user should retry rather than launch Pane (launching with a mismatched UUID risks ADR-0026 take-over confusion).
5. **Restore `.pane/system/`** — if extraction fails, log warning and continue. State will reconcile from gateway on next OC connection.
6. **Restore keychain** — if import fails, log warning and continue. State will reconcile via Settings cascade-UX prompts when the customer next pairs.
7. **Done** — return `Ok(RestoreSummary { warnings: Vec<String> })` with non-fatal warnings collected for UI display.

The summary type allows the UI (#971) to render "Restore complete" with a yellow-warning banner when the warnings vec is non-empty, vs. a green-success banner when it's empty.

---

## Error class enumeration (for #971)

The contract surfaces these specific error/warning classes for #971 to render. Names are spec-canonical; implementation uses Rust idioms.

### Fatal errors (return `Err`)

| Class | Trigger | UI handling |
|---|---|---|
| `SchemaTooNew { backup: i64, current: i64 }` | Step 2: `backup > current` | Block dialog: "This backup was created with a newer version of Pane (schema {backup}). Update Pane to a version that supports schema {backup} or higher and try again." Single CTA: [Update Pane] (links to Pane's update page). No "Restore anyway" path. |
| `UserCancelled` | Step 2: user clicks Cancel on older-schema confirmation | UI returns to file-picker / pre-restore state. No state touched. |
| `DbRestoreFailed(String)` | Step 3: SQLite restore error | Block dialog with error string. Single CTA: [Try a different backup]. |
| `InstallUuidRestoreFailed(String)` | Step 4: write failure | Block dialog: "Couldn't restore install UUID. Migration cannot complete without this. Try restoring again, or contact support." CTA: [Try again] [Cancel]. |

### Non-fatal warnings (collected in `RestoreSummary.warnings`)

| Class | Trigger | UI handling |
|---|---|---|
| `KeychainRestorePartial(String)` | Step 6: keychain import error | Yellow banner on Home: "Some credentials weren't restored. Re-enter via Settings → Providers." Persistent until user acks or re-pairs. |
| `KeychainRestoreFailed(String)` | Step 6: total keychain import failure | Same as above with stronger copy: "No credentials were restored. Re-enter via Settings → Providers before continuing." |
| `SystemFilesRestoreFailed(String)` | Step 5: tar extraction failure | Yellow banner: "Identity files weren't restored. They will re-sync when you connect to OpenClaw." (Only shown if customer's restored configuration includes OC.) |

The `RestoreSummary.warnings` field accumulates one entry per non-fatal failure. Empty vec = clean restore; non-empty = success-with-warnings. UI uses `len()` to choose banner color.

### Schema-older confirmation (returns to UI before mutation)

| Class | Trigger | UI handling |
|---|---|---|
| `SchemaOlderConfirmation { backup: i64, current: i64 }` | Step 2: `backup < current` | Confirmation dialog before any state mutation: "This backup was created with an older version of Pane (schema {backup}). Current version is schema {current}. Pane will migrate the backup data when you launch the app. Continue?" CTAs: [Restore anyway] [Cancel]. |

The UI awaits the user's choice and then re-invokes restore with a `confirm_older_schema: true` flag (or equivalent) to bypass the confirmation on retry. Implementation detail; the spec is the user-visible contract.

---

## Why install UUID is fatal but keychain isn't

This is the load-bearing distinction; both are credentials, but the failure modes differ.

**Install UUID restore is fatal because the failure is non-recoverable from the user's side at restore time.** ADR-0026 makes `install_uuid` the source-of-truth for `active_machine_id` matching with ecom. If the backup contains UUID `A` and the new machine generated UUID `B` on first launch (which happens before restore), and the restore fails to overwrite `B` with `A`, then:

- Ecom sees a request from UUID `B` against an account whose `active_machine_id = A`.
- The take-over prompt fires (correctly, by ecom's lights — a different machine is asking for active status).
- The customer accepts take-over, ecom updates `active_machine_id = B`.
- The original machine, when next online, sees `active_machine_id != B` and signs out.
- Net effect: silent eviction of the source machine, potentially mid-session if the customer was actively using both machines during migration.

The user has no UI surface that says "your UUID didn't restore" unless we make it a hard error. Surfacing it as fatal forces the user to retry the restore (transient FS errors usually resolve on retry) before launching Pane, which preserves ADR-0026's take-over semantics.

**Keychain restore failure is recoverable because Settings cascade-UX exists.** §22C.11A.3 already handles missing-credential states gracefully:

- Missing gateway_token → cascade-UX surfaces "Re-pair OpenClaw" prompt with the inline pairing flow (§22C.11A.5).
- Missing tls_cert_pin → re-fetched on next gateway connection via `/v1/health`.

The customer can resume work without keychain restore succeeding; the UX surfaces the gap and the recovery path. Soft warning is the right call.

---

## UX implications for #971

The restore UI (sub-task #971) consumes this contract. Concrete requirements:

- **Pre-restore**: file picker, then schema check happens before any state mutation. UI shows a spinner during step 1-2.
- **SchemaTooNew → block dialog** with [Update Pane] CTA. Modal, no Continue path.
- **SchemaOlderConfirmation → confirmation dialog** before restore proceeds. [Restore anyway] [Cancel].
- **DbRestoreFailed / InstallUuidRestoreFailed → block dialog** with retry option.
- **Restore success** drops into Home (per §6.4) regardless of warnings. Warnings render as a persistent yellow banner above the Home content; user can dismiss after acknowledging or after re-pairing/re-entering credentials.
- **No partial-progress UI** during restore (the operation is fast enough — DB restore + ~3 sidecar reads — that a progress bar isn't needed). Spinner with "Restoring..." copy is sufficient.

The summary type returned by `restore_backup` (success path):

```rust
pub struct RestoreSummary {
    pub warnings: Vec<RestoreWarning>,
    pub backup_metadata: BackupMetadata,  // for Home banner copy
}
```

The error type (failure path):

```rust
pub enum RestoreError {
    SchemaTooNew { backup: i64, current: i64 },
    SchemaOlderConfirmationRequired { backup: i64, current: i64 },
    UserCancelled,
    DbRestoreFailed(String),
    InstallUuidRestoreFailed(String),
}
```

These are spec-canonical names; the actual Rust types may use slightly different forms (Result + non-exhaustive enums for forward-compat). The contract is the user-visible behavior; implementation details are #971 / PR #1006's call.

---

## Spec text changes

One concrete edit to `docs/spec/onboarding.md` §6.1.4. The new sub-section §6.1.4.1 is inserted after the existing §6.1.4 body and before the §6.2 heading.

### Insert new sub-section after §6.1.4

```markdown
##### 6.1.4.1 Restore error contract

> **Status:** SPEC LOCKED 2026-05-07. NOT BUILT — sub-tasks #1006 (backend
> error-handling fix) and #971 (restore UI) consume this contract.

The restore flow has fatal errors and non-fatal warnings, distinguished
by whether the failure is recoverable from the user's side at restore
time.

**Step ordering (load-bearing):**

1. Read backup metadata (`_metadata.json`).
2. Schema check: `backup.schema > current` is fatal (block); `backup.schema < current` requires confirmation; `backup.schema == current` proceeds silently.
3. Restore DB.
4. Restore install UUID — fatal on failure (ADR-0026 take-over breaks otherwise).
5. Restore `.pane/system/` — soft on failure (re-syncs from gateway).
6. Restore keychain — soft on failure (recoverable via Settings cascade-UX).

The schema check must precede any state mutation. Steps 4-6 mutate state
only after schema compatibility is established.

**Fatal errors (block restore, return `Err`):**

- `SchemaTooNew` — backup was created with a newer Pane version. UI shows upgrade-Pane message; no "Restore anyway" path.
- `DbRestoreFailed` — SQLite restore failed. UI shows error; user retries or picks a different backup.
- `InstallUuidRestoreFailed` — UUID write failed. UI shows error; user retries before launching Pane (launching with mismatched UUID corrupts ADR-0026 take-over semantics).

**Non-fatal warnings (continue restore, surface in summary):**

- `KeychainRestorePartial` / `KeychainRestoreFailed` — keychain items
  missing. User re-enters via Settings cascade-UX (§22C.11A.3).
- `SystemFilesRestoreFailed` — `.pane/system/` extraction failed. Files
  re-sync from gateway on next OC connection.

**Schema-older path:** When `backup.schema < current`, restore is paused
for user confirmation before any state mutation. User-cancellation at
this prompt is not an error class; it returns the UI to its pre-restore
state.

Full spec including error-class enumeration and UX requirements: see
`docs/spec-prep/2026-05-07-pr1006-spec-decisions/`. The restore UI
(sub-task #971) consumes this contract.
```

---

## References

- Reed's audit: `docs/audits/2026-05-07-pr1006-backup-audit.md` Finding 2
- Pane source: `src-tauri/src/backup.rs:618-681`
- ADR-0026: `docs/adr/0026-install-uuid-active-machine-id.md` §6 (install UUID is launch-blocking for take-over)
- Spec: `docs/spec/onboarding.md` §6.1.4 (parent — backup-restore offer); §22C.11A.3 (cascade-UX recovery for missing credentials); §22C.11A.5 (inline pairing flow)
- Companion amendment: `2026-05-07-adr-0024-amendment-backup-encryption.md` (sibling deliverable in this session)
- Sub-tasks: #1006 (PR — backend error-handling fix), #971 (restore UI — consumes this contract; on hold until this amendment lands)
- Out of scope tracking: Reed's PR #1006 Findings 3 (rotation cleanup), 4 (keychain discovery), 5 (parent dir creation — merge-blocking, not spec), 6 (test isolation) — separate follow-up issues per the audit's recommendation
