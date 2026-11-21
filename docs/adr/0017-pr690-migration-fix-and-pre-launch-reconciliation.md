# ADR-0017: PR #690 Migration Fix and Pre-Launch Mode Reconciliation

**Status:** Proposed
**Date:** 2026-04-30
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (clarifies and reconciles `docs/spec/file-system.md` §2B.2 and §2B.3)

## Context

PR #690 (transcript archive feature, commit `4015735`, landed before v0.64.0 on 2026-04-28) bumped the canonical schema constant `SCHEMA_VERSION` from `"61"` to `"62"`, added `chat_archive` and `chat_archive_fts` tables to `migrations.rs::create_chat_tables()` with `IF NOT EXISTS` guards, and shipped without a v61→v62 migration. The PR was reviewed against the pre-launch model documented in `docs/spec/file-system.md` §2B.3, which says "if the DB schema doesn't match the canonical definition, delete and recreate."

That delete-and-recreate path does not exist in code. `migrations.rs::run_migrations` returns a `SCHEMA_MISMATCH:N->M` error string on version mismatch; `database.rs::init_database` catches that error per §2B.2's "graceful migration failure" rule and continues with the existing schema. No deletion. No recreation. The behavior matches §2B.2 (added in v0.15.0 incident response: "A failed migration MUST NEVER cause data loss; the DB is NEVER recreated if it contains data") but not §2B.3 (added 2026-04-18 in v0.61.0: "if the DB schema doesn't match the canonical definition, delete and recreate").

Net effect on existing v0.63.x users who upgrade to v0.64.x or v0.65.x: the schema-version stamp stays at v61 forever, the new `chat_archive` and `chat_archive_fts` tables are never created on existing DBs, and `[CRITICAL] Migration failed: SCHEMA_MISMATCH:61->62` fires on every app launch. Backups still run successfully on every launch (§2B.1 was not affected). No user data is lost or corrupted. Fresh installs are fine — `init_schema` runs once on a fresh DB and creates v62 tables with the correct stamp.

The full incident timeline, gate-by-gate analysis of why each gate missed it, and residual-risk discussion are in `docs/audits/2026-04-30-pr690-migration-incident.md`. This ADR resolves two structural decisions that came out of that postmortem:

1. **The migration fix.** What ships to existing-install users so the v62 feature is actually available and the `[CRITICAL]` log stops firing.
2. **The pre-launch mode reconciliation.** Either §2B.2 or §2B.3 has to give. They contradict each other; the implementation has been quietly following §2B.2 for two days while the spec said §2B.3.

### What if we didn't decide

For the migration fix: Pane runs in production at v61 on every existing DB forever, with the chat archive feature inaccessible to anyone who upgraded rather than fresh-installed. The `[CRITICAL]` log fires on every launch. Each subsequent schema bump compounds the gap — by the time someone notices and fixes it, multiple "missed migration" cases have accumulated and Option A's lift-and-shift simplicity is gone.

For the spec reconciliation: PR #690's failure mode is not a one-time bug; it is the predictable output of two contradictory spec sections. Any future PR that bumps the schema version will hit the same gap. Reed audits PRs against spec; if the spec says two opposite things, "PR matches spec" is a vacuous claim. The next bump produces the same incident.

## Decision

### Part 1 — Migration fix: Option A (write the actual v61→v62 migration as an idempotent table-creation pass)

The v62 schema additions (`chat_archive`, `chat_archive_fts`, plus their indexes) already exist in `migrations.rs::create_chat_tables()` with `CREATE TABLE IF NOT EXISTS` and `CREATE VIRTUAL TABLE IF NOT EXISTS` guards. The fix is to call them on the v61→v62 mismatch path and stamp the version, instead of returning the error string. Lift-and-shift, not original code.

Implementation shape (`migrations.rs::run_migrations`, mismatch branch):

```rust
Some(ref old_version) if old_version == "61" && SCHEMA_VERSION == "62" => {
    eprintln!("[MIGRATIONS] Schema version v61 → v62: applying idempotent table additions");

    // Backup first (§2B.1 already runs above; this is belt-and-suspenders for the
    // migration-specific path)
    let db_path = crate::database::get_db_path();
    if let Err(e) = crate::backup::create_migration_backup(&db_path, "v61-to-v62") {
        eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
        // Continue — §2B.1's launch-time backup already ran successfully or we
        // wouldn't have reached this code.
    }

    // Apply v62 additions. All guarded with IF NOT EXISTS so re-runs are safe.
    create_chat_archive_tables_v62(conn)?;
    verify_crr_tables(conn)?;

    set_schema_version(conn, SCHEMA_VERSION)?;
    eprintln!("[MIGRATIONS] ✓ v61 → v62 migration applied");
}
```

`create_chat_archive_tables_v62` is extracted from the existing `create_chat_tables` so the same SQL definitions cover both fresh-install and migration paths. No new schema, no new indexes — exactly the `chat_archive` table, the `chat_archive_fts` virtual table, and the two indexes that PR #690 added.

The mismatch branch is **explicit per version pair**, not generic. A v60→v62 upgrade or a v61→v63 upgrade falls through to the existing SCHEMA_MISMATCH error path. Pre-launch we ship few enough versions that explicit per-pair handling is tractable, and explicit-per-pair forces the schema-bumping engineer to decide what each pair does. This is intentional friction and aligns with the post-launch direction (a real migration chain).

Why Option A over the alternatives:

- **Option B (revert PR #690).** Feasible — chat archive is launch-blocker per memory architecture decisions (2026-04-23) but not yet wired into UI. Rejected because (a) the underlying spec contradiction would still exist for the next bump, (b) reverting gives up §5.14.2 work that is otherwise correct, and (c) revert plus re-land is more PRs and more chances to repeat the same failure mode.
- **Option C (bump to v63 with no-op v61→v62 and v62→v63).** Encodes the failure into migration history. Each future engineer reading the migration code sees a "no-op for the bug we shipped" entry that requires reading this postmortem to understand. Worse for clarity than Option A.

The migration fix lands as a single PR. Includes the migration code, a unit test that constructs a v61-stamped DB and verifies the v62 tables exist + version is stamped after `run_migrations`, and a §4 integration test (per ADR-0016) that exercises the full upgrade path through `init_database`. CHANGELOG entry references this ADR and #794.

### Part 2 — Pre-launch mode reconciliation: keep §2B.2's behavior, remove §2B.3's contradicting language

§2B.2 was added after the v0.15.0 data-loss incident: graceful migration failure, never recreate the DB. That rule was the right post-v0.15.0 fix and remains the right rule. The implementation has followed it correctly for the entire post-v0.15.0 period.

§2B.3's "delete and recreate on schema mismatch" framing was added 2026-04-18 (v0.61.0) when the canonical-schema-no-migration-chain model was introduced. The framing was wrong — it described an aspirational pre-launch shortcut that never got built — but it stayed in the spec for 12 days and was the basis for PR #690's review.

Three options were considered for the reconciliation:

1. **Remove §2B.3's aspirational language. Make §2B.2's behavior (graceful fallthrough, never destroy) the documented behavior. Add explicit-per-version-pair migration paths in `run_migrations` for the pre-launch period.**
2. **Build the safe destructive version with explicit guards (backup-first verification, debug-build vs release-build gating, schema-version-downgrade check).**
3. **Restrict pre-launch mode to non-distributed builds (CI, test, internal-only) so the destructive path can't reach a user's machine even if it gets built.**

**Path 1 is selected.** Reasoning:

- **Path 2 (safe destructive)** rebuilds the thing §2B.2 was added specifically to prevent. The v0.15.0 incident was caused by destruction-on-migration-failure; building it back with guards reintroduces the same failure surface and depends on the guards being correct. The cost-benefit is wrong: the alternative (explicit per-pair migrations) is simpler than designing destruction guards that are reliably safe.
- **Path 3 (restrict by build type)** is operationally fragile. "This binary is not distributed" is hard to enforce mechanically; debug builds get shared with Matt routinely (the mechanism that surfaced this incident). Build-type gating that protects against accidental destruction would have to be strong enough that even a debug build refuses to destroy a user's DB, at which point it's path 1 with extra steps.
- **Path 1 (graceful fallthrough as documented behavior)** matches what the code has been doing for two days while the spec said otherwise, requires no new code path, and aligns with §2B.2's data-loss-prevention principle. The cost is doing migration work properly per version pair instead of relying on delete-and-recreate. That cost is small at pre-launch volumes (2 versions in the migration chain so far, both well-understood) and has the additional benefit of being directly continuous with the post-launch model (real migrations that actually run).

Spec changes that ship with this ADR:

- **`docs/spec/file-system.md` §2B.2.** Update to reflect that "graceful fallthrough" is the production behavior. Add language: "When a version mismatch is detected, an explicit per-version-pair migration is applied. If no migration exists for the pair, the SCHEMA_MISMATCH error is logged and the app continues with the existing schema (the v62 feature, in that case, is unavailable until a migration ships). The `[CRITICAL]` log surfaces the gap loudly per ADR-0018's CI smoke check."
- **`docs/spec/file-system.md` §2B.3.** Strike the "delete and recreate" language. Replace with the explicit-per-pair migration model. Note that the pre-launch model differs from a full migration chain only in that the canonical schema is the post-migration target; per-pair migrations bridge old DBs forward to that target.
- **`migrations.rs` comments.** Strike "Pre-launch: delete + recreate on version mismatch (§2B.3)". Replace with a comment that documents the actual behavior: "Per-version-pair migrations applied on mismatch; SCHEMA_MISMATCH error surfaced if no pair handler exists. Never destroys data per §2B.2."

The postmortem (`docs/audits/2026-04-30-pr690-migration-incident.md`) is the scar-tissue record. The §2B.3 update references it.

## Consequences

### Positive

- **Existing-install users get the v62 feature on the next release.** `chat_archive` and `chat_archive_fts` tables get created on next launch, schema stamps to v62, the `[CRITICAL]` log stops firing.
- **The §2B.2/§2B.3 contradiction is resolved.** Future schema bumps follow one model, not two contradictory ones. Reed's PR review against the spec produces a non-vacuous verdict.
- **Pre-launch model becomes continuous with the post-launch model.** Per-pair migrations are how real migrations work. Pre-launch differs only in being authored against a moving canonical target instead of an immutable history. The "switch from no-chain to chain" event at launch becomes a removal of the canonical-schema fallthrough, not a migration-system-rewrite.
- **Lift-and-shift implementation.** v62 SQL definitions already exist in `create_chat_tables`. The migration is moving them, not writing them. Lower bug surface, smaller PR.
- **Aligns with §2B.2's safety principle, which has been correct since v0.15.0.** Never destroy data on migration failure. The new spec language reinforces what the implementation already does.

### Negative

- **Per-version-pair migration entries in `run_migrations`.** Each schema bump now requires an explicit branch in the match statement. At pre-launch volumes (low single digits) this is fine; at higher volumes it would scream for refactoring into a registry. Mitigation: when the count gets uncomfortable (rough threshold: 5+ pre-launch pairs), refactor to a `Vec<MigrationStep>` with version-from/version-to/SQL fields. Not now.
- **Slight friction added to schema bumps.** Bumping `SCHEMA_VERSION` now requires writing the migration branch, not just adding tables to `init_schema`. This is correct friction — the friction was missing in the broken model — but it's friction.
- **Spec edits are non-trivial.** §2B.3's framing leaked into other spec sections that reference the canonical-schema-no-migration-chain model. A grep pass needs to find them and update — small but real cleanup work.

### Risks

- **Engineer forgets to add the migration branch when bumping `SCHEMA_VERSION`.** Same shape as PR #690. Mitigation: ADR-0018's CI smoke check catches the `[CRITICAL]` log on the next launch. Additional mitigation: a unit test that asserts every version-pair branch exists for every version step from the lowest currently-supported version to `SCHEMA_VERSION`. Land this test alongside the migration fix.
- **The v61→v62 migration runs against a DB that's already at v62 somehow.** Idempotent `IF NOT EXISTS` guards make this safe, but the version-stamping step would overwrite a hypothetical v62-stamped DB with v62 (no-op). Mitigation: the match arm only runs when `old_version == "61"`. A v62-stamped DB hits the `version == SCHEMA_VERSION` branch and skips the migration entirely.
- **`backup::create_migration_backup` failure on the v61→v62 path.** §2B.1's launch-time backup already ran successfully (otherwise we wouldn't reach the migration code). The migration-specific backup is belt-and-suspenders; failure logs but does not block migration. Same posture as the existing mismatch branch.
- **Per-pair model becomes unwieldy faster than expected.** If the team starts churning schema, "5+ pairs threshold" arrives quickly. Mitigation: weekly playbook review (existing cadence) flags the count; refactor to registry when the count crosses the threshold rather than continuing to add branches.
- **Spec contradictions resurface elsewhere.** §2B.2 vs §2B.3 wasn't a unique event; the spec has 12 domain files that grew in parallel. Mitigation: this ADR doesn't try to fix that broader risk. The §11 system audit cadence is the existing answer; the PR #690 incident is added to the audit-findings reference list.

## Alternatives Considered

**Option B — Revert PR #690 entirely.** Roll canonical schema back to v61. Lose chat archive functionality until properly specified and built. Cleanest if archive isn't load-bearing. Rejected because the underlying spec contradiction would still exist; the next schema bump produces the same incident; chat archive (§5.14.2) is launch-blocker work per memory architecture decisions; revert plus re-land is more PRs, more reviews, more chances to repeat the failure.

**Option C — Bump to v63 with explicit no-op v61→v62 and v62→v63.** Acknowledge the broken bump, neutralize it, move forward. Rejected because it encodes the failure into migration history. Future engineers reading the migration code see a "no-op for the bug we shipped" entry whose context lives in the postmortem; explicit per-pair migration in Option A is cleaner and the v62 work is genuinely useful (the tables are needed; the bug was that they didn't get created on existing DBs).

**Path 2 — Build safe destructive pre-launch mode.** Backup-first verification, debug-build gating, schema-downgrade check. Rejected per the analysis above: rebuilds what §2B.2 prevented, requires correctness in the guards, and the alternative (per-pair migrations) is simpler.

**Path 3 — Restrict pre-launch mode to non-distributed builds.** Mark the destructive path debug-only or test-only. Rejected because debug builds get shared with Matt routinely and the gate would need to be strong enough that debug builds also refuse to destroy. At that point it's path 1 with extra steps.

**Defer the spec reconciliation; ship only the migration fix.** Land Part 1, leave §2B.3's framing intact for now. Rejected because the spec contradiction is the structural cause of the incident. Fixing only the symptom guarantees a re-occurrence on the next bump. Both parts ship together.

## References

- Postmortem: `docs/audits/2026-04-30-pr690-migration-incident.md` — full incident record
- Spec sections affected: `docs/spec/file-system.md` §2B.2 (graceful migration failure — language reinforced), §2B.3 (pre-launch schema management — "delete and recreate" language struck, replaced with explicit-per-pair migration model)
- Code paths: `src-tauri/src/migrations.rs::run_migrations` (mismatch branch added), `src-tauri/src/migrations.rs::create_chat_archive_tables_v62` (extracted from `create_chat_tables`), `src-tauri/src/database.rs::init_database` (graceful-fallthrough behavior reinforced via spec, no code change)
- Related ADRs: ADR-0006 (Canonical Schema, No Migration Chain — Pre-Launch — superseded by ADR-0019); ADR-0019 (Pre-Launch Schema Management Clarification — the framing fix this ADR's spec reconciliation depends on; filed together with this ADR); ADR-0018 (CI smoke check for startup logs — the detection mechanism that catches the next §2B.3-style failure); ADR-0008 (mechanical dev hygiene — pattern this ADR follows); ADR-0016 (mechanical §4 enforcement — the migration fix's integration test runs through this layer)
- Reference incident: PR #690 (commit `4015735`), Issue #794
- Related historical incident: v0.15.0 migration data-loss event (`docs/spec/file-system.md` §2B.5) — origin of §2B.2's "never recreate" rule
