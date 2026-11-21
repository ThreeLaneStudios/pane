# ADR-0061: Smart Routing Is Global-Only — Per-Project Routing Columns Retired

*Line citations predate the #3097 module split.*

**Status:** Accepted
**Date:** 2026-08-16
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (ADRs untouched; spec sections §27.2, §27.3, §27.6.1a and the §27 status banner are superseded under #2534)
**Supplements:** ADR-0037 (Decision 7 extends §3's decorative-picker rule to the full dark-config family, via the clarifying-supersession path 0037's own Risks section prescribed), ADR-0008 (bidirectional round-trip check filed as a mechanization candidate)
**Related:** ADR-0038 amendment 2026-05-19 (per-feature lookup; the derived-enablement contract in Decision 4 is consistent with it)

## Context

On 2026-08-16 Matt reproduced three smart-routing states in a single build: Symbiotic routed manual/Opus, while Pane Marketing and Home routed Haiku via global settings. The resolver behaved correctly per spec — the defect was data. Symbiotic carried an explicit `projects.smart_routing_enabled = 0` that no UI can set, see, or clear. Execution-log evidence fixed the §29 boundary: the terminal trace shows the classifier request dispatched via OpenRouter direct, with the response model confirming Haiku. Classifier and simple-tier dispatch live on the internal side; the gateway is never in the path.

**Reed audit 1** (`docs/audits/2026-08-16-smart-routing-scope.md`, #2531) proved the mechanism. Commit `8a7f6a7c` (v0.32.3, "Smart routing is now global only") deleted the entire per-project AI Model Settings card — routing toggle, tier pickers, and the Default Model dropdown — but left the frontend `$state`, the NULL-collapse on load (`settings.smart_routing_enabled || false`), and the unconditional save in place. Every Project Settings save since v0.32.3 has written `0` into the row, silently hard-disabling routing for that project. The always-send behavior originated in `14bb1f79` (v0.32.1), which replaced send-if-changed with send-always. This is the only production write path; every non-NULL value in the wild is indistinguishable from collateral.

**Reed audit 2** (`docs/audits/2026-08-16-router-model-scope.md`, #2533) established the neighboring columns. `projects.router_model` is an orphaned read: zero write sites have ever existed — no command parameter, no frontend state, no INSERT, no migration — yet the spec names it the top-priority classifier model source. That priority ordering is unconfigurable fiction. `projects.simple_tier_model` has a guarded send (`|| null`) so it takes no spurious writes, but it is live-but-invisible: pre-v0.32.3 values still steer routing and cannot be seen, changed, or cleared in any UI. `projects.default_model` shares that shape — guarded send, live reads on the manual-selection and classifier-complex paths, its dropdown removed in the same `8a7f6a7c` card. A dedicated write-site grep at the pinned SHA (session source-read round 3, quoted verbatim in session notes) closed the gate: exactly one production writer, `handleSave` line 337 via `update_project_settings`; global `ai_settings.model` is a separate, live key and is untouched by this ADR.

**The session source-read at `e931465617a9406c620f4eeba9f65d10e44953e0` (2026-08-16) extended the family into `ai_settings`:**

- `tier_1_model` is an orphaned read. Zero writers exist (one test seed only). Nine reads, all `.or_else()` legacy fallbacks off global `simple_tier_model`, inside `resolve_model_for_message`'s simple-tier response-model resolution. A stray key in an old database silently resurrects configuration the customer believes is gone.
- The global `ai_settings.smart_routing_enabled` key is never read. Enablement is derived: smart routing runs iff global `simple_tier_model` is non-empty AND an internal provider is configured (`resolve_model_for_message`, Step 1). The Settings toggle-off path already operates by clearing `simple_tier_model`.
- `tier_2_model`, `tier_3_model`, and a global `router_model` key have zero production references. The §27.2 five-key global list is real on exactly one key: `simple_tier_model`.
- Correction to audit 2 §5: at this SHA, per-project `simple_tier_model` is read as the simple-tier response model as well (`resolve_model_for_message`, project branch), not classifier-only. The retirement decision is unchanged; the mechanism description is corrected for the record.

The product decision "smart routing is now global only" has existed since v0.32.3 only as a code comment. The spec still documents the removed per-project UI (§27.3), still specifies the fictional classifier chain (§27.6.1a), and still carries a `NOT BUILT / #384` status banner over a section that is substantially built. #2517 is the team-reported symptom that opened this arc; #2534 is the spec pass this ADR anchors.

## What if we didn't decide

- Every Project Settings save keeps silently hard-disabling routing per project — the #2517/#2531 class recurs indefinitely.
- The spec keeps directing customers at unconfigurable configuration, including the live error copy that tells users to set `router_model`.
- Stale pre-v0.32.3 `simple_tier_model` and `default_model` values keep invisibly steering model selection.
- The `tier_1_model` fallback keeps a path open for deleted configuration to resurrect.
- Global-only remains a code comment, reversible by accident in any refactor.

## Decision

### 1. Smart routing is global-only. Ratified.

One smart-routing configuration per install. Per-project routing overrides are retired as a product capability, and no per-project routing UI will be reintroduced. (Matt, 2026-08-16, final; ratifies the `8a7f6a7c` intent.)

### 2. Four `projects` columns are retired: `smart_routing_enabled`, `router_model`, `simple_tier_model`, `default_model`.

Retired means: after the code fix, zero production read sites and zero production write sites. The columns remain physically in the schema — SQLite column drops require a table rebuild, and retirement is code-path removal, not DDL. Test fixtures may continue declaring them as historical schema.

### 3. Data migration, shipping in the same release as the code fix.

```sql
UPDATE projects
SET smart_routing_enabled = NULL,
    router_model          = NULL,
    simple_tier_model     = NULL,
    default_model         = NULL;

DELETE FROM ai_settings
WHERE key IN ('tier_1_model', 'tier_2_model', 'tier_3_model',
              'router_model', 'smart_routing_enabled');
```

Constraint analysis, per the enumeration requirement: the `projects` table's only constraint is `PRIMARY KEY (id)`; zero `UNIQUE` indexes exist on `projects` anywhere in the codebase (source-read of `migrations.rs` at the pinned SHA); all four columns are nullable with no `NOT NULL` or `CHECK` constraints. An UPDATE-to-NULL on non-key columns cannot collide on any constraint. The `ai_settings` DELETE removes rows by primary key match and cannot collide. External testers exist: this is a real migration under the post-2026-07-24 policy, and the smoke-harness pass condition applies.

### 4. Enablement contract: derived, not stored.

Smart routing is enabled iff global `simple_tier_model` is non-empty AND an internal provider is configured. No boolean enable key exists, and none will be added. Disabling smart routing means clearing `simple_tier_model` — the existing Settings toggle-off behavior. This is consistent with ADR-0038's per-feature gate (each feature gates on its own model key alone) and with ADR-0037 §2a (the AI tab is smart routing's feature home).

This is the **post-fix contract**. At the pinned SHA the derived rule already governs every path except the retired per-project override (explicit `smart_routing_enabled = 1`), which Decision 2 removes — the current-reality trace is in the session notes.

### 5. Classifier and simple-tier model resolution collapse to the global key.

Both the classifier model and the simple-tier response model resolve from global `simple_tier_model` only. The chain is: global `simple_tier_model` → feature silently disabled per the §27.6.1 matrix (chat falls through to the primary provider, complex tier). No per-project levels. No `tier_1_model` fallback.

### 6. The scope survives the provider roadmap.

Under per-session agent binding and runtime-per-agent, routing configuration remains global — one smart-routing config per install — with the internal provider as its own standalone connection config. This decision binds the scope (global), not the current Settings-page shape. The derived-enablement contract transfers directly: global `simple_tier_model` set AND the internal connection configured. (Matt, 2026-08-16.)

### 7. ADR-0037 §3 is extended to the full dark-config family.

ADR-0037 banned decorative pickers: UI writing a key nothing reads (`rerank_model`). This arc surfaced the remaining shapes of the same defect:

- **Orphaned write** — code writes a field no UI controls (`projects.smart_routing_enabled`).
- **Orphaned read** — code reads a field nothing can write (`projects.router_model`, `ai_settings.tier_1_model`).
- **Live-but-invisible** — stale data steers behavior with no UI surface (`projects.simple_tier_model`, `projects.default_model`).

The generalized rule, scoped to **user-facing configuration**: **no schema column or `ai_settings` key holding user-facing configuration may be production-readable without a live write surface, or production-writable without a live UI control. Removing a UI surface must retire the full read/write plumbing and migrate the stored data in the same change.** System-managed state — onboarding flags, embeddings status, sync bookkeeping, migration markers, display caches such as `ai_settings.model` under OC (§27.6.1b) — is exempt: its writer is code, and the check names the owning subsystem instead of a UI control. PR review's round-trip check (0037 §3) becomes bidirectional: every reader names its writer; every writer names its UI control or its system owner. This is the clarifying-supersession path ADR-0037's own Risks section prescribed when per-project override tension surfaced.

## Rationale

**Why global-only.** The per-project capability has been unreachable since v0.32.3. Four months of releases produced nothing from its remains except defects — three distinct dark-config shapes across five fields. If per-context routing granularity ever returns, its natural home is the per-session binding / runtime-per-agent architecture, where the unit is the session's agent, not a project row. Ruling globally now removes the whole family at no real cost.

**Why NULL every row rather than preserve legitimate values.** Legitimate pre-v0.32.3 values are indistinguishable from post-v0.32.3 collateral without per-row audit trails that do not exist (audit 1 §5b). NULL restores every project to the only state the current UI can express: inherit global.

**Why delete the `ai_settings` keys rather than leave them inert.** `tier_1_model` proves stray keys are not inert — a legacy fallback read silently resurrected configuration paths four months after the UI died. The DELETE plus the code PR's fallback removal closes both ends, and Decision 7 prevents new fallbacks from forming.

**Why derived enablement, with no boolean key.** A stored boolean can drift from the keys that actually gate dispatch — the exact drift family this ADR closes. Deriving enablement from the operative key means the display, the gate, and the data cannot disagree.

**Why extend ADR-0037 now.** Three instances in one arc is a family, not a coincidence, and 0037's Risks clause pre-authorized exactly this supersession shape.

## Consequences

### Positive

- The #2517/#2531 defect class is eliminated at both the data layer and the code layer, not patched around.
- Spec §27 matches shipped reality, and the global-only ruling is durable rather than a code comment.
- One key (`simple_tier_model`) is the entire smart-routing configuration surface; round-trip verification becomes trivial.
- The migration is collision-free by construction.

### Negative

- Any customer who legitimately set per-project values before v0.32.3 loses them silently. Accepted: those values have been invisible and uncontrollable for four months, and surviving values are overwhelmingly collateral.
- An install that today reaches smart routing only through the retired per-project path — explicit `smart_routing_enabled = 1` with global `simple_tier_model` unset, response model supplied by the stray `tier_1_model` fallback — loses smart routing until global `simple_tier_model` is configured. Accepted and intended: that is the only configuration in which the fallback can fire at the pinned SHA (derived enablement already requires a non-empty global `simple_tier_model`, so the fallback is unreachable on every derived path), the fallback's own log line said "Please reconfigure in Settings," and the post-change behavior is the standard silently-disabled gate — chat falls through to primary.
- Test fixtures still declare the retired columns. Mitigated by §27.2's retirement note marking them historical.

### Risks

- A hidden frontend writer of the deleted `ai_settings.smart_routing_enabled` key recreates the key post-migration. Harmless — zero readers — but it is a decorative writer under Decision 7. The code PR carries a grep-and-remove checklist item, and the Settings smart-routing toggle must display derived enablement rather than read the deleted key.
- Future demand for per-context routing overrides. Path: a new ADR in the per-session-binding world, per 0037's Risks clause. This ADR forecloses per-project columns, not future per-session design.

## Alternatives Considered

**Reintroduce the per-project UI and make the columns honest.** Rejected. The product ruling is global-only, and the `8a7f6a7c` comment's "backend support preserved for future re-enablement" plan is precisely what produced four months of dark config.

**Preserve non-NULL values through the migration.** Rejected. Indistinguishable from collateral (audit 1 §5b).

**Drop the columns physically.** Rejected. SQLite column drops require a table rebuild — all risk, no benefit versus code-path retirement, and table rebuilds on a cr-sqlite-synced schema are their own hazard class.

**Keep `tier_1_model` as a compatibility fallback.** Rejected. It violates per-feature single-key lookup (ADR-0038 amendment) and Decision 7. A warning log line is not a UI surface.

**Spec-only cleanup, no migration.** Rejected. Poisoned rows would stay poisoned with no way to clear them — the entire point of #2531.

## References

- `docs/audits/2026-08-16-smart-routing-scope.md` — Reed, #2531: write-site enumeration, `8a7f6a7c` UI-removal history, NULL-collapse + unconditional-send mechanism, §5 recommendations
- `docs/audits/2026-08-16-router-model-scope.md` — Reed, #2533: `router_model` zero-write proof, `simple_tier_model` guard analysis, classifier-chain reality. Correction recorded in Context: per-project `simple_tier_model` also feeds the simple-tier response model at the pinned SHA, not classifier-only.
- Session source-read 2026-08-16 @ `e931465617a9406c620f4eeba9f65d10e44953e0` (Brit fetch): `tier_1_model` write-site enumeration; `resolve_model_for_message` lines 1020–1165; `smart_routing_enabled` grep of `commands.rs`; `CREATE UNIQUE INDEX` enumeration (zero on `projects`); `commands.rs` `get_ai_setting` key census
- Session source-read round 3, 2026-08-16 @ the same SHA (Brit grep, quoted verbatim in session notes): `projects.default_model` full write-site enumeration — exactly one production writer (`ProjectSettings.svelte` `handleSave` line 337, guarded `|| null`, via `update_project_settings` at `commands.rs:15014`); production reads at `commands.rs:1192` (resolver manual path), `:6902` (classifier complex path), `:14928` (`get_project_settings`); global `ai_settings.model` is a separate, live key, untouched
- Matt's terminal trace, 2026-08-16: classifier request dispatched via OpenRouter direct, response model Haiku — execution-log evidence per the §29 boundary-trace requirement
- Commits: `ad2cc226` (v0.25.1, per-project routing UI ships), `14bb1f79` (v0.32.1, always-send regression origin), `8a7f6a7c` (v0.32.3, UI removed, "Smart routing is now global only")
- Issues: #2517 (team-reported symptom), #2531 (orphaned write), #2533 (router_model / simple_tier_model audit), #2534 (spec pass consuming this ADR), #2536 (Settings-page session removal — related, separate)
- ADR-0037 (§3 extended by Decision 7; Risks clause prescribing this supersession path), ADR-0038 amendment 2026-05-19 (per-feature lookup), ADR-0008 (mechanization precedent)
- `docs/spec/smart-routing.md` §27 status banner, §27.1, §27.2, §27.3, §27.6.1a; `docs/INVARIANTS.md` Smart Routing (§27) — superseded/amended under #2534

## Follow-ups filed (not gated by this ADR)

- **Code fix PR (#2531 / #2533), same release as the migration:** remove `smartRoutingEnabled` from the `handleSave` payload with tri-state cleanup per audit 1 §5a, and sweep the dead picker machinery for `defaultModel` and `simpleTierModel` — state, `$effect` sync, reset and diff logic at `ProjectSettings.svelte` lines 150–263, 286–290, 306, 325, 410–422 per the session grep — not just the payload fields; remove all resolver reads of the four retired `projects` columns (including `default_model` at `commands.rs:1192` and `:6902`); remove the `tier_1_model` fallbacks; fix the classifier-missing error copy (currently directs users to set `router_model`); grep for and remove any frontend writer of the `ai_settings.smart_routing_enabled` key; converge the Settings smart-routing toggle on derived enablement.
- **Post-launch CI mechanization** per ADR-0008: bidirectional round-trip check (writer ↔ reader ↔ UI control), extending the check filed under ADR-0037 §3.