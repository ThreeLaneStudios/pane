# ADR-0063: Runtime-Per-Agent — Global Provider Mode Retired

**Status:** Proposed
**Date:** 2026-08-16
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** ADR-0059 D1 (active-class usability and local-agent class-float), D2 (provider-aware `is_main` selection rule), D3 (switch-path contract) and its 2026-08-03 #2392 amendment; ADR-0045 D3 (OC authoritative on the flag while active); agents.md §24.5.5's trigger/mode framing (import semantics preserved — see D6); §24.7C in full
**Preserves:** ADR-0045 D1/D2 (single flag, demote-then-promote in one transaction — renamed, same shape); ADR-0059 D4 (name-commit transaction boundary, re-shaped per D6), D5 (attribution semantics, already restructured by ADR-0062), D6 (no deletion); ADR-0061 D6 (routing global; internal provider is its own standalone connection)
**Supplements:** ADR-0044 (keychain credential storage — per-runtime credentials ride it), ADR-0022 (per-device posture unchanged)
**Related:** #2522, #2536, ADR-0062 (phase 1 — prerequisite), docs/audits/2026-08-16-per-session-binding-prespec.md @ 9c96a5fd

## Context

Phase 1 (ADR-0062) makes the agent a session property but leaves the global provider mode live: one active class at a time, out-of-class sessions read-only. This ADR retires the mode itself. The audit's BREAK verdict enumerates why the current architecture cannot host it: `AiConfig` is one struct for the whole process (§1a), the frontend `StreamManager` is a module singleton with unconditional cancel-on-start (§1e), model fetching populates from whichever single provider the global config holds (§1d), and `is_main` is physically incapable of representing two simultaneously-active agents — a unique partial index at `agents.rs:784` plus five ambient lookup sites (§2).

Locked decisions (Matt, 2026-08-16): runtimes connected once, all live simultaneously; dispatch per session via session → agent → runtime, OC-gateway and API-direct concurrently; agents permanently belong to their runtime; local agents pin provider + model at creation; the global provider setting dissolves into per-runtime connection configs plus a default agent for new sessions; one gateway pairing per install; provider auto-switch as a session-picker side effect is rejected.

## Decision

### D1 — Agents permanently belong to a runtime: `runtime_type` + `runtime_ref`

`ai_agents` generalizes its OC-only identity:

- **New column:** `runtime_type TEXT` — domain `'openclaw' | 'openrouter' | 'anthropic' | 'ollama'`, matching the existing `ai_settings.provider` string values (extensible: Venice adds `'venice'` later, out of scope here). Assigned at creation, immutable — an agent never changes runtime (locked).
- **Rename:** `oc_agent_id` → `runtime_ref` via `ALTER TABLE ... RENAME COLUMN` (no rebuild; SQLite rewrites index references automatically). Semantics generalize: *the agent's identity within its runtime's own registry*. `'openclaw'` rows carry the OC agent id exactly as today; API-direct runtimes have no agent registry, so their rows carry NULL. Pane-side rename only — the gateway process and every wire contract are untouched. Rename mechanics are plain DDL — CRR analysis at D4 (non-CRR table; the rename-on-CRR hazard is recorded there).
- **Index re-scope:** `DROP INDEX idx_ai_agents_oc_id_unique;` then `CREATE UNIQUE INDEX idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);` Runtime refs are namespaced by runtime — two future runtimes may each have a `"main"`. NULLs are distinct under SQLite unique indexes, so locals coexist freely, exactly as before.
- **Model pinning:** local (API-runtime) agents require `model` (existing column) at creation and dispatch with it. OC agents' `model` remains the synced display value; the gateway keeps resolving the real model from its agent registry (#1453's decoupling, restated in D9).

**Semantic-trap ruling (audit §2):** `sync_client`'s `oc_agent_id == "main"` comparisons (`sync_client.rs:877, 1147, 2257`; gateway-side echoes) are OC's *own* `"main"` slug in OC's namespace — sync-file allowlisting, nothing else. They become `runtime_ref == "main"` gated on `runtime_type == 'openclaw'`, and were never, and must never be, read as Pane's default-agent flag. The two "main"s are unrelated by contract.

### D2 — All runtimes live simultaneously; dispatch is session → agent → runtime

Connection is a one-time act per runtime, not a mode. OC-gateway and API-direct streams run concurrently. Every dispatch resolves: `session.agent_id` → agent row → (`runtime_type`, connection config, `model` or gateway-resolved). No dispatch decision reads a global provider — the `ai_settings.provider` key retires (D8).

**One gateway pairing per install is explicit v1 scope.** Multi-gateway is not representable (`gateway_tls_cert_pem` is one DB row — audit §1b) and stays out.

Smart routing is untouched: global-only, internal provider as its own standalone connection config (ADR-0061 D6). The internal channel never touches session/agent/runtime resolution.

### D3 — `is_main` becomes `is_default`: the default agent for new sessions and sessionless surfaces

`ALTER TABLE ai_agents RENAME COLUMN is_main TO is_default;` then `DROP INDEX idx_ai_agents_main; CREATE UNIQUE INDEX idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;` (rename auto-rewrites the old index; the swap is for the name; plain DDL per D4's CRR analysis).

- Semantics: **exactly one default**, DB-enforced, of any runtime. It preselects the session-creation picker, binds the three auto-create paths, and answers the sessionless background surfaces (D5). It is a Pane-side user preference.
- Change mechanism: demote-then-promote in one transaction — ADR-0045 D1/D2 preserved verbatim under the new name (`set_main_agent_impl` renamed accordingly). "Set as Main" becomes "Set as Default," offered for **any active agent of any runtime** — the class gate (ADR-0059 D2) dies with the class.
- **OC import never re-targets the default** — ADR-0045 D3 retired. OC remains authoritative over OC agents' *content and roster* (import/deactivate); the default flag is Pane's.
- **Deactivation edge:** if the default agent is deactivated (OC-side removal), re-target to the most recently created active agent (`created_at DESC`) in the same transaction as the deactivation; surface an error if zero active agents exist (a state ADR-0059 D4's no-main guarantee makes unreachable in normal operation).

### D4 — Migration (real arm, post-2026-07-24 policy)

`SCHEMA_VERSION` bump at build (a later slot than ADR-0062's v78). Ordered steps, one migration — plain SQLite DDL throughout; the CRR analysis below establishes no alter-protocol wrapping is required, and its escalation path governs if the membership confirm contradicts it:

1. `ALTER TABLE ai_agents RENAME COLUMN oc_agent_id TO runtime_ref;`
2. `ALTER TABLE ai_agents ADD COLUMN runtime_type TEXT;`
3. Backfill: `UPDATE ai_agents SET runtime_type = 'openclaw' WHERE runtime_ref IS NOT NULL;` Locals (`runtime_ref IS NULL`): if the pre-dissolution active provider is an API class, pin them to it — that is the runtime that has been animating them, truthfully. If the active provider is `'openclaw'` at migration time, locals were last animated by an unrecorded API provider: they take `runtime_type = NULL` (legacy-unpinned) and **pin on first selection** — the picker prompts a one-time runtime choice and writes it. DDL-nullable; every creation path writes it; NULL is legal only as this migration legacy.
4. `DROP INDEX idx_ai_agents_oc_id_unique; CREATE UNIQUE INDEX idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);` — collision-free by construction: existing data is unique on `runtime_ref` alone, so it is unique on the pair.
5. `ALTER TABLE ai_agents RENAME COLUMN is_main TO is_default;` + index swap (D3).
6. `ai_settings` dissolution per D8 (key deletes ride the same migration, ADR-0061 D3 pattern).

No row deletes, no table rebuild, no file moves. `notes.agent_id` and `chat_messages_local.agent_id` reference `ai_agents.id`, which never changes.

**CRR membership & alter-protocol analysis (the #2538 rigor — Matt amendment, 2026-08-16):**

1. **Membership.** `ai_agents` is **not a CRR table** on the source logic: cr-sqlite's compatibility gate rejects any table carrying a non-PK unique index ("has unique indices besides the primary key. This is not allowed for CRRs" — `is_table_compatible`, `core/rs/core/src/tableinfo.rs:909-925` @ upstream main, fetched 2026-08-16), and `ai_agents` carries **three** (`slug UNIQUE` inline, `idx_ai_agents_main` partial unique, `idx_ai_agents_oc_id_unique`). `crsql_as_crr('ai_agents')` would error. **This contradicts the working presumption that `ai_agents` is row-synced.** Its actual sync surface is the gateway *file* sync plus import/reconstitution — INVARIANTS: "Filesystem is source of truth for agent identity files; DB is an index." **Letter-of-record confirm — blocking micro-fetch for Brit before the spec PR branches:** `migrations.rs:32-57` (`CRR_TABLES`) verbatim; grep `crsql_as_crr` / `crsql_begin_alter` / `crsql_commit_alter` across `src-tauri/`; the pinned cr-sqlite version from `Cargo.lock` (the compatibility cite is upstream-main; re-check at the pin if anything diverges).
2. **DDL mechanics (non-CRR, per the above).** Every step in the ordered list is plain SQLite DDL — no `crsql_begin_alter`/`crsql_commit_alter` wrapping, and both `RENAME COLUMN` steps are safe as drafted. **For the record, were the table CRR, the renames would be affirmatively unsafe, not merely unproven:** `crsql_commit_alter`'s compaction keys clock entries by `col_name` and deletes every entry whose name is absent from the post-alter schema (`compact_post_alter`, `core/rs/core/src/alter.rs` @ upstream main — the `DELETE FROM "{tbl}__crsql_clock" WHERE "col_name" NOT IN (SELECT name FROM pragma_table_info(...))` pass), so a rename destroys the column's causal history. The fallback in that world is add-copy-retire — ADD the new column, triggered-UPDATE copy (writes fresh clocks), code-path-retire the old column (ADR-0061 D2 pattern), index swap, all inside one begin/commit wrapper (index DDL rides in the same wrapper) — specced here per instruction, but it **cannot coherently apply**: a CRR `ai_agents` is impossible under upstream rules given its own existing unique indexes, and this migration *adds* one more. If the membership fetch nonetheless shows `ai_agents` in `CRR_TABLES`, the vendored cr-sqlite diverges from upstream — **halt, escalate to Matt, and redo this analysis against the pinned source before the spec PR.**
3. **Cross-device.** No CRR sync surface → the backfill UPDATEs have no LWW dimension, and no migrated/unmigrated device window exists — nothing syncs a `runtime_type` value toward a device lacking the column. The one real cross-device fact: agent **rows** are per-device state reconstituted from gateway-synced identity files, so in a future multi-device world the device-local judgments here (`runtime_type` pin from the local pre-dissolution provider; the `is_default` choice) can diverge across devices. Accepted window, #2538-style: v1's single-machine posture (onboarding §6 — the same posture ADR-0059 D5 leans on). Multi-device agent-state semantics are a future ADR when multi-Pane lands.
4. **Companion tables, one line each (matching #2538's `ai_settings` treatment):** `chat_sessions` — non-CRR, local-only, no sync surface (ADR-0062 D3.5). `chat_messages_local` — non-CRR (`migrations.rs:1453`), local-only, no sync surface. `ai_settings` (D8's key deletes) — non-CRR per #2538's own analysis; the deletes are local-only.

### D5 — Sessionless surfaces resolve the default; the ADR-0062 D7 rule completes

The rule ("session agent where a session is in scope; designated default otherwise") is unchanged; this ADR swaps the designated default's identity from `is_main` to `is_default`. Consolidator and sessionless workspace-path callers read `is_default = 1 AND is_active = 1`. No other ambient lookups exist post-phase-1 (INVARIANTS).

### D6 — Switching machinery retired; import semantics survive as connection actions

- **§24.5.5's trigger dies** (`provider === "openclaw" && savedProvider !== "openclaw"` — there is no saved provider). OC agent discovery/import becomes a **connect/refresh action on the OC runtime connection**. The two-phase command split (read-only collision check → confirmed mutation), overwrite-in-place via UPDATE-never-DELETE, coexist + disambiguate on slug collision (numeric-increment-until-free), fresh import, and the warn-and-confirm modal all survive verbatim — they are data semantics, not mode semantics.
- **ADR-0059 D3's switch matrix retires** (no transition to enumerate). Its create flow survives re-shaped: agent creation is always available; the flow picks a runtime (among connected runtimes) and, for API runtimes, a model; name-commit remains the transaction boundary (D4 preserved) at which the agent row commits — with no coupled provider write, since there is none. The #2392 amendment (creation disabled while OC active) retires with the class gate.
- **§22C.11A.4's key wipe re-scopes** from "switch away from OC" to **explicit OC disconnect** — a deliberate user action on the OC connection block. Nothing wipes on connecting an additional runtime.
- Out-of-class read-only sessions (ADR-0062 D5) cease to exist as a class matter: every session dispatches via its own agent's runtime. The read-only state survives only for the narrow cases of a deactivated bound agent or a disconnected runtime — same derived-state treatment, same error contract.

### D7 — Concurrency demolition (phase-2 build entry criteria)

The three structural blockers (audit §1) are the entry gate for this phase's build:

1. **`AiConfig` singleton** → per-dispatch resolution from session → agent → runtime connection. No process-global provider/model slot survives; `ai_configure_and_save`'s role narrows to per-runtime connection config writes.
2. **`StreamManager`** → per-thread map (`Map<threadId, StreamState>`); cancel is per-thread; the unconditional cancel-on-start (`streamManager.ts:205`) is removed. Event dispatch matches on the event's own stream id, not a single stored one.
3. **Model fetch** → per-runtime population (`fetch_available_models` takes a runtime, or iterates connected runtimes). `ModelRegistry` storage is already provider-namespaced (audit §1d) — storage unchanged.

The incidental base_url double-read snapshot fix already landed in phase 1 (ADR-0062 D8).

### D8 — Settings dissolution: global keys retire, per-runtime connection configs replace them

Structural shape (visuals are Claude Design's, downstream):

- **Runtime Connections** — one block per runtime: OC (gateway pairing — one per install), OpenRouter, Anthropic, Ollama. Each independently connect/test/disconnect; all may be connected simultaneously.
  - **Correction (2026-08-30, #2777 venice live-test scoping):** ADR-0065 D8 originally extended this line's "one block per runtime, always" precedent to "renders five cards, always, connected or not" (Venice being the fifth runtime added there). Both that extension AND this line's own "always all runtimes" precedent are superseded: `ProviderConnectionsPanel` now renders a card only for a runtime that is actually configured this install (`configured_this_install`); an unconfigured runtime has no card at all, and "+ New Provider" is the sole entry point for connecting one. This note exists so a future reader of this ADR in isolation (without also reading ADR-0065) doesn't reinstate the old "always all runtimes" assumption. See ADR-0065 D8 for the corrected card-visibility text.
- **Default Agent** — the `is_default` picker (D3).
- **Smart Features** — unchanged; the internal provider is its own standalone connection config beside the runtime blocks (ADR-0061 D6).
- **No sessions, no chat** (#2536 — precondition already landing this sprint).

**Key dispositions** (grep-first census in session notes; migration deletes ride D4 step 6):

| Key | Disposition |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| `provider` | Retired — no reader survives D2/D7. Row deleted. |
| `endpoint` | Retired as a global; Ollama's endpoint moves to **new key `ollama_endpoint`** (only API runtime with a configurable endpoint; naming follows the `internal_endpoint` per-channel precedent). Migration copies the value when the pre-dissolution provider was `'ollama'`. |
| `api_key` | Retired as a global slot; per-runtime credentials live in the keychain per ADR-0044. Exact keychain entry naming is build-scope under 0044's contract. |
| `model` | Retired as a global (see D9). Row deleted after the D9 re-target. |
| `internal_*`, `simple_tier_model`, `gateway_*`, memory/embeddings keys | Untouched. |

Per ADR-0061 D7, every surviving key must name its writer and UI control; the new `ollama_endpoint` writer is the Ollama connection block.

### D9 — `ai_settings.model` retires; the OC display cache moves to the agent row

Under runtime-per-agent, the dispatch model is per-agent (D1), so the global `model` key has no dispatch-adjacent meaning. Its one live role — the OC resolved-model display cache (#1453; `X-OC-Resolved-Model` write-back) — **re-targets the session's agent row** (`ai_agents.model` of the OC agent that answered), which is where OC agent models already live via the registry sync. The #1453 invariant is preserved in per-agent form: the cache remains display-only and decoupled from dispatch (the gateway still resolves from its own registry). Settings and AI-Health display surfaces read the relevant agent row.

**Build note (Matt, 2026-08-16):** the write-back writes the agent row **only when the resolved value differs from the stored `ai_agents.model`** — never per response. Identical-value rewrites are pointless churn: `updated_at` noise on the identity table for certain, and CRR clock churn in any world where the table is row-synced. D4's analysis finds it is **not** row-synced (the presumption is contradicted at source); the compare-before-write ships regardless and is correct in either world.

*Accepted (Matt, 2026-08-16); the conditional INVARIANTS edit (Smart Routing block) applies.*

## Constraint-collision enumeration (`ai_agents` — this ADR changes its INSERT/UPDATE behavior)

| Constraint | Columns | Collision decision |
| ------------------------------------------------------------ | ---------------------- | ------------------------------------------------------------ |
| `PRIMARY KEY (id)` | `id` | Fresh UUID per insert; collision is a UUID-reuse bug → error/rollback. Renames/backfills touch no key. |
| `slug UNIQUE` (inline; `idx_ai_agents_slug` is non-unique lookup) | `slug` | Coexist + disambiguate, numeric-increment-until-free — §24.5.5's rule, unchanged, now applying identically to import and to the always-available create flow. Never a rollback. |
| `idx_ai_agents_main` → `idx_ai_agents_default` — `UNIQUE(is_default) WHERE is_default = 1` | `is_default` (partial) | Demote-then-promote in one transaction (ADR-0045 D2 shape, preserved). A violated ordering is a hard constraint error/rollback, never silent. Migration renames only; cardinality unchanged. |
| `idx_ai_agents_oc_id_unique` → `idx_ai_agents_runtime_unique` — `UNIQUE(runtime_type, runtime_ref)` | pair | Import overwrite-in-place matches on `('openclaw', ref)` → UPDATE in place (§24.5.5, unchanged). Fresh import inserts a new pair. Locals write NULL `runtime_ref` → distinct under SQLite, cannot collide. Index swap in D4.4 is collision-free (data already unique on `runtime_ref` alone). |
| `idx_ai_agents_active`, `idx_ai_agents_owner` | non-unique | Not constraints; no decision required. |

Backfill interactions: D4.3's UPDATE writes only `runtime_type` (non-key, not yet uniquely indexed at that step — the composite index is created after backfill); cannot collide.

## Consequences

**Positive:** the #2522 premise (OC and API-direct concurrently) becomes representable end to end — schema, dispatch, stream layer. Provider switching, the single largest source of agent-identity defects (#956, #1492, #1527, #1546, #2012, #2030 arc), stops existing as a category. "Which agent, which runtime, which model" is stored fact at every layer.

**Negative:** the largest migration since sync — two column renames, one column add, two index swaps, key deletions, all on the identity table. Mitigated: no rebuild, no row deletes, each step independently idempotency-guardable, and the smoke harness gates it. Legacy-unpinned locals (openclaw-active-at-migration edge) carry a one-time picker prompt.

**Risks:** any code path still reading `ai_settings.provider` post-migration is an orphaned read (ADR-0061 D7 shape) — the code PR carries a grep-and-remove checklist item, same as 0061's. `Settings.svelte`'s class gate (`agent.is_main && provider === 'openclaw'`, audit §2) dies structurally with the provider string; Reed's scope must confirm removal rather than letting it rot into a dark predicate. A future runtime with its own agent registry (Hermes, if applicable) consumes `runtime_ref` as designed — no further schema motion.

## Alternatives considered

**Keep `oc_agent_id` untouched, add `runtime_type` only** — rejected: leaves the generalized concept split across a misleading name and a new column, and leaves the single-column unique index wrong the moment a second registry-bearing runtime exists. The rename is rebuild-free and Pane-side only.

**New `default_agent_id` ai_settings key instead of the renamed column** — rejected: trades a DB-enforced exactly-one constraint and an existing atomic transaction shape for an unenforced pointer that can dangle — the drift family ADR-0061 D4 just closed.

**Provider-scoped default (`UNIQUE(is_default, runtime_type)`)** — rejected: the picker needs one preselection, not one per runtime; ADR-0045 D1's original single-flag reasoning holds.

**Physical drop of retired global keys' readers only, keys left inert** — rejected: `tier_1_model` proved stray keys resurrect (ADR-0061 rationale); deletes ride the migration.

## References

- docs/audits/2026-08-16-per-session-binding-prespec.md @ 9c96a5fd — §1a/1b/1c/1d/1e (blockers), §2 (`is_main` incompatibility, semantic trap, Settings.svelte predicate), §3b (schema), §3d (dispatch), §3e (global-provider read census)
- ADR-0044 (keychain), ADR-0045, ADR-0052, ADR-0059 (+#2392 amendment), ADR-0061 (D3 migration pattern, D4 derived state, D6 routing scope, D7 dark-config rule), ADR-0062 (prerequisite)
- agents.md §24.2, §24.5.5, §24.7A/B/C, §24.10.1; sessions.md §30.7; sync.md §22C.11A.4; INVARIANTS.md "Agent Identity," "Smart Routing (§27)"
- #2522, #2536, #2538 (CRR-analysis rigor precedent), #956/#1492/#1527/#1546/#2012/#2030 (the defect lineage this retires)
- cr-sqlite upstream @ main (fetched 2026-08-16): `core/rs/core/src/tableinfo.rs:909-925` (`is_table_compatible` bars non-PK unique indexes from CRRs); `core/rs/core/src/alter.rs` (`compact_post_alter` deletes clock entries whose column names are absent post-alter — rename destroys causal history); README:73-80 (`crsql_begin_alter`/`crsql_commit_alter` protocol)
