# ADR-0019: Pre-Launch Schema Management Clarification

**Status:** Accepted
**Date:** 2026-04-30
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** ADR-0006 (Canonical Schema, No Migration Chain — Pre-Launch)

## Context

ADR-0006 introduced the pre-launch schema model on 2026-04-18: a single canonical schema definition (`init_schema()`), no migration chain, "delete and recreate" on version mismatch. The intent was sound — eliminate the failure class of multi-step migration chains, keep the schema definition in one editable place, accept data loss during pre-launch development since there are no customers to protect. ADR-0006 captured that decision correctly for the moment it was made.

The framing in ADR-0006 was ambiguous in two ways that produced drift between spec and implementation, and the drift surfaced as the PR #690 incident on 2026-04-30 (postmortem: `docs/audits/2026-04-30-pr690-migration-incident.md`):

1. **"No migration chain" was read as "no migration code on schema bumps."** The phrase was intended to mean "no permanent ordered chain of migration files accumulating through history" — the v0.61.0 problem ADR-0006 was solving. PR #690's review (and the spec language at `docs/spec/file-system.md` §2B.3 that ADR-0006 anchored) interpreted it as "no per-version migration work needed at all; bump the schema constant and rely on delete-and-recreate." That interpretation produced a PR that bumped `SCHEMA_VERSION` from `"61"` to `"62"`, added new tables to `init_schema()`, and shipped without any code path for existing v61 DBs to reach the v62 state.

2. **"Delete and recreate" described a code path that was never built.** ADR-0006 §Decision step 2 says "If mismatch: delete the DB and recreate from canonical schema." The implementation never wired that up. `migrations.rs::run_migrations` returns a `SCHEMA_MISMATCH:N->M` error on version mismatch; `database.rs::init_database` catches the error per `docs/spec/file-system.md` §2B.2's "graceful migration failure" rule (added after the v0.15.0 incident) and continues with the existing schema. The implementation has been doing the safer thing (never destroy data) since v0.15.0; ADR-0006's framing said the destructive thing would happen. Both contradicted, the implementation was correct, the spec and ADR were misleading.

The PR #690 incident exposed the drift. ADR-0017 fixes the immediate bug and reconciles `docs/spec/file-system.md` §2B.2 and §2B.3. This ADR completes that work at the ADR layer: ADR-0006's framing needs the same correction so a future reader loading ADR-0006 without ADR-0017 in context — exactly the case during onboarding or a §11 system audit — doesn't reproduce the same drift.

ADR-0006 cannot be edited in place; the Dev Hygiene Playbook §2 prohibits modifying accepted ADRs. The supersession mechanism is the prescribed path. ADR-0006 stays as a historical record; ADR-0019 carries the corrected framing forward.

### What if we didn't decide

Two predictable failure modes:

- **The drift re-forms.** A future engineer reading only ADR-0006 picks up the original "no migration chain → no per-version work" reading and ships a PR with the same shape as PR #690. The detection layer (ADR-0018's startup smoke check) catches the symptom, but the root cause — the framing that produced the wrong mental model — isn't addressed at its source.
- **ADR-0017's reference is doing structural work in a parenthetical.** ADR-0017's References section currently includes "ADR-0006 (Canonical Schema..., clarified by this ADR; the no-migration-chain framing meant 'no full chain through history,' not 'no per-pair migration work')." That clarification is load-bearing; burying it as a parenthetical inside ADR-0017's References section means it gets read only by people already reading ADR-0017. The clarification needs to be first-class — its own ADR — so it surfaces during ordinary ADR navigation.

## Decision

The pre-launch schema model is updated as follows. ADR-0006's intent is preserved; the framing that produced drift is corrected.

**Pre-launch schema management uses a single canonical schema definition (`init_schema()`) as the post-migration target. Schema bumps require explicit per-version-pair migration code in `run_migrations()` to bridge older DBs forward to the canonical target.** The canonical schema is a moving target; the per-pair migration mechanism is identical to the post-launch migration mechanism, differing only in that pre-launch the canonical target is editable while post-launch it freezes into immutable history.

Concretely, this restates ADR-0006 as:

1. **One canonical schema**, exactly as ADR-0006 said — `init_schema()` is the single source of truth for what the schema *is* at the current `SCHEMA_VERSION`.
2. **No permanent migration chain accumulating through history** — no migration files numbered `001_initial.sql`, `002_add_column.sql`, etc., piling up forever. This was ADR-0006's actual target and remains correct.
3. **Per-version-pair migrations are required when bumping `SCHEMA_VERSION`.** Bumping from N to N+1 requires adding a match arm in `run_migrations()` that brings v(N)-stamped DBs to the v(N+1) state. Idempotent (`IF NOT EXISTS` for table creation, no-op on already-applied state). The arm lives in the same PR as the schema bump.
4. **`docs/spec/file-system.md` §2B.2's graceful-fallthrough behavior is the production behavior on missing per-pair migrations.** If `run_migrations()` encounters a mismatch with no matching arm, it logs the SCHEMA_MISMATCH error and `init_database()` continues with the existing schema. The new feature is unavailable; data is preserved. ADR-0018's CI smoke check fails the build before merge if this path produces a `[CRITICAL]` log on either fresh or upgrade fixtures, which prevents the PR-#690-shape from shipping again.
5. **Delete-and-recreate is not the model.** ADR-0006's step 2 ("If mismatch: delete the DB and recreate from canonical schema") is replaced by step 3 above. The implementation has been doing the right thing per §2B.2 throughout; this ADR brings the model into agreement with the implementation.

The post-launch model (real migration chain) is unchanged in scope from ADR-0006's "Post-launch, a proper migration system will be built (#492)." The migration mechanism itself — explicit per-version-pair code — is now the same in both periods. The transition at launch becomes "freeze the canonical schema; from this point forward, per-pair migrations accumulate as immutable history" instead of "switch from no-migrations to migrations." Smaller, simpler transition.

## Consequences

### Positive

- **The drift that produced PR #690 is corrected at the source.** Future readers loading ADR-0019 instead of ADR-0006 (via the supersession banner) get the framing that matches the implementation. Reed audits PRs against a spec model that doesn't contradict itself.
- **Pre-launch and post-launch models become continuous.** The migration mechanism is the same; only the target's mutability differs. The launch transition is a smaller event than ADR-0006's framing implied.
- **Immutability rule preserved.** ADR-0006's body stays unedited. Historical record intact. The supersession mechanism handles the correction without violating the playbook.
- **Cheap to file.** This ADR is short — most of the substantive work was done in ADR-0017. ADR-0019 makes the framing fix first-class instead of subordinate to ADR-0017's References section.

### Negative

- **One more ADR file in the index.** Not free; ADR navigation has one more entry. Mitigated by the supersession banner — readers seeing ADR-0006 are forced to ADR-0019, which keeps the cognitive load on whoever is actively reading rather than on everyone who might.
- **"Per-pair migration entries" land in `run_migrations()`.** ADR-0017 already captures this consequence; restated here so ADR-0019 is self-contained. At pre-launch volumes (low single digits) this is fine. At ~5+ pairs the match statement screams for a registry refactor; that's a future ADR, not this one.

### Risks

- **A reader bypasses both ADR-0006 and ADR-0019 and reads only the spec.** `docs/spec/file-system.md` §2B.2 and §2B.3 are updated by ADR-0017 to match this framing; that's the third leg of the correction. If the spec is also out of sync, the same drift is possible. Mitigation: ADR-0017's spec edits land alongside this ADR. The §11 audit cadence catches drift between ADR and spec going forward.
- **The supersession banner gets missed.** A reader scrolling past the status line of ADR-0006 reads the body without realizing it's been superseded. Mitigation: status banners in this codebase are at the top of the file, before any other content; the convention is well-established. Worst case, the reader picks up the original framing; ADR-0018's CI smoke check catches the resulting bug at PR time. Detection-in-depth.
- **Future "clarifying" supersessions abuse the mechanism.** If every contested reading produces a clarifying ADR, the supersession chain grows long and traceability suffers. Mitigation: ADR-0019 is filed because the original ADR's framing produced a documented, reproducible incident. The bar for clarifying-supersession is "framing demonstrably produced drift in production," not "a reader was confused once." Future clarifying supersessions should cite their own incident or be declined.

## Alternatives Considered

**Edit ADR-0006 in place with a clarifying note.** The most direct path: append a "Clarification (2026-04-30)" section to ADR-0006 explaining the framing fix. **Rejected** because it violates the Dev Hygiene Playbook §2's immutability rule for accepted ADRs. The rule exists for audit traceability and historical-record reasons; bending it for a "small clarification" is exactly how immutability rules erode. The supersession path produces the same reading experience (banner → updated content) without violating the rule.

**Leave ADR-0006 unamended; rely on ADR-0017's parenthetical reference.** Skip ADR-0019 entirely. **Rejected** because future readers loading ADR-0006 in isolation — onboarding contexts, §11 audits — see the original framing without the corrective lens. The drift can re-form. The PR #690 incident is exactly what happens when ADR framing diverges from implementation; leaving the divergence in place invites recurrence.

**Mark ADR-0006 as Deprecated rather than Superseded.** Same effect on the index, slightly different signal — deprecation suggests the decision is no longer relevant; supersession suggests the decision was correct in intent and is now expressed more accurately. **Rejected** because ADR-0006's intent was correct; only the framing produced drift. Supersession captures that nuance; deprecation overstates the reversal.

**Defer to a broader spec-vs-ADR audit.** Wait until the §11 audit on the migration system runs (which would naturally surface the §2B.2/§2B.3 contradiction and ADR-0006's framing together) and address all of it in one pass. **Rejected** because the next §11 audit is unscheduled and the drift is active now. Filing this ADR alongside ADR-0017 keeps the correction tight against the incident that exposed it; deferring lets the drift continue producing risk during the audit interval.

## References

- Superseded ADR: ADR-0006 (Canonical Schema, No Migration Chain — Pre-Launch). Status updated to `Superseded by ADR-0019` as part of this filing; body unchanged per immutability rule.
- Reference incident: PR #690 (commit `4015735`), Issue #794. Postmortem: `docs/audits/2026-04-30-pr690-migration-incident.md`.
- Companion ADR: ADR-0017 (PR #690 Migration Fix and Pre-Launch Mode Reconciliation). Implements the per-pair migration code, reconciles `docs/spec/file-system.md` §2B.2 and §2B.3. ADR-0019 carries the framing fix at the ADR layer; ADR-0017 carries it at the code and spec layers. Filed together.
- Detection mechanism: ADR-0018 (CI Smoke Check for Startup Log Severity). Catches PR-#690-shape failures before merge regardless of whether the framing drift recurs.
- Dev Hygiene Playbook §2 (Architecture Decision Records). The supersession-not-amendment rule that this ADR follows. The rule's exact phrasing: "ADRs are immutable once accepted. If the decision changes, write a new ADR that supersedes the old one. Never edit the original."
- Post-launch migration tracking: #492 (unchanged from ADR-0006).
