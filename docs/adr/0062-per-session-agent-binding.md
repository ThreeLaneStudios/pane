# ADR-0062: Per-Session Agent Binding

*Line citations predate the #3097 module split.*

**Status:** Proposed
**Date:** 2026-08-16
**Amended:** 2026-08-17 — D2’s project-born-active row superseded: project creation binds by initiator (user → New Project dialog picker, default preselected; agent tool → acting agent). Only lazy-create and hide-fallback remain silent-default. See Amendment 2026-08-17 at bottom of file.
**Amended:** 2026-08-23 — D5 scope clarification: append-in ≠ dispatch-out. External message ingestion (§30.11) is not dispatch; locked/out-of-class agents still accept ingested messages. See Amendment 2026-08-23 at bottom of file.
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** sessions.md §30 intro's "provider is resolved per-turn and never bound to a session" (locked 2026-07-08 — the agent half of that ruling is reversed; per-turn *provider* resolution survives phase 1); agents.md §24.8.2 (derive-at-save attribution) in full; ADR-0059 D5's stamp *mechanism* (its NULL-legacy semantics stand verbatim); the chat-switch role of ADR-0045 D1–D3's `set_main_agent` machinery (the transaction shape survives; its effect on existing conversations ends)
**Supplements:** ADR-0052 (session identity — binding rides the same table), ADR-0053 (compaction keys on session id, unaffected), ADR-0059 (in-class gating unchanged within this phase), ADR-0061 D6 (routing stays global; untouched)
**Related:** #2522 (roadmap anchor), #2248 (closed by elimination — see D4), #2536 (Settings chat removal, locked precondition), docs/audits/2026-08-16-per-session-binding-prespec.md @ 9c96a5fd (source-of-record)

## Context

Nothing partial exists to extend. The pre-spec audit proved no column, parameter, or query carries a session-to-agent relationship anywhere between the frontend `invoke()` and the provider HTTP call (audit, Cross-references). Every dispatch decision resolves from the global `AiConfig` singleton or global `ai_settings` keys (audit §3d: five decision points, zero session/agent inputs), and one global `is_main` flag answers "which agent" for every surface (audit §2: five ambient lookup sites with zero session parameter).

The live defect this phase fixes: `agent_id` derives at save time from whoever holds `is_main` after the stream ends (`commands.rs:9478-9525`), so a mid-stream agent change mis-attributes the message. §24.8.2 ratified that window as a documented limitation with #2248 as its exit. Binding removes the window structurally instead.

Product decisions locked by Matt, 2026-08-16: agent is a property of the session, chosen at creation, immutable after; phase 1 is in-class agents only with the global provider mode still live; attribution becomes structural.

## Decision

### D1 — The agent is a property of the session, immutable after creation

`chat_sessions` gains `agent_id TEXT REFERENCES ai_agents(id)` (declared, unenforced — FK enforcement is off in this codebase; consistency by contract + migration + tests, matching §30.1.2 and the v76 `chat_messages_local.agent_id` precedent).

Immutability is mechanical: **no update path exists.** No command writes `chat_sessions.agent_id` after INSERT. A different agent means a new session. Re-binding is rejected as a concept, not guarded against at runtime.

### D2 — Creation paths: picker on the explicit path, designated default on the three auto paths

The audit enumerates exactly four INSERT sites (§3c). One is user-explicit; three are automatic:

| Path | Site | Binding |
| ------------------------------------ | ------------------ | ------------------------------------------------------------ |
| `create_session` ("New Chat") | `commands.rs:9722` | Gains a required `agent_id` parameter. Picker at creation on Home and every project; the designated default is preselected. Picker offers in-class agents only in phase 1 (§24.7C unchanged). |
| Project born-active | `projects.rs:122` | Designated default, no UI. |
| `get_active_session` lazy-create | `commands.rs:9994` | Designated default, no UI. |
| `hide_session` last-visible fallback | `commands.rs:9915` | Designated default, no UI. |

**Designated default in phase 1 = the `is_main = 1 AND is_active = 1` row** — unchanged machinery. ADR-0063 renames and re-semanticizes it; this ADR only narrows what it *governs* (D6).

### D3 — Migration: additive column, backfill to current main, self-heal contract

`SCHEMA_VERSION` 77 → 78 (v77 — ADR-0061's arm — is merged; this arc takes the next slot, renumbered at build only if an intervening migration lands first). Real migration arm per the post-2026-07-24 policy; smoke-harness pass condition applies.

1. `ALTER TABLE chat_sessions ADD COLUMN agent_id TEXT REFERENCES ai_agents(id);` — purely additive; MUST NOT drop, recreate, rename, or copy-and-swap the table (chat-session rows are customer data, same discipline as §24.8.1).
2. Backfill: `UPDATE chat_sessions SET agent_id = (SELECT id FROM ai_agents WHERE is_main = 1 AND is_active = 1) WHERE agent_id IS NULL;`
3. **Edge — backfill target missing** (no `is_main = 1 AND is_active = 1` row; a corrupt state ADR-0045's Dependencies section documents as historically possible): fall back to the most recently created active agent (`is_active = 1 ORDER BY created_at DESC LIMIT 1` — the same default rule as ADR-0059 D3's picker). If zero active agents exist, rows stay NULL and the self-heal contract below governs.
4. **NULL self-heal contract:** DDL-nullable (SQLite `ALTER ADD COLUMN` cannot take a dynamic NOT NULL default); semantically NOT NULL after migration — every INSERT provides it. Any reader encountering NULL post-migration resolves the designated default, writes it back, and logs distinctively (`[SESSION] BINDING SELF-HEAL:`). This is the §30.2.6 lazy-safety-net posture applied to binding: mechanized recovery, never silent ambiguity.
5. **CRR membership (the #2538 rigor — Matt amendment, 2026-08-16).** `chat_sessions` is **not a CRR table**: its own creation function says so in live-fetched source (`migrations.rs:1449-1488` @ 9c96a5fd — doc comment "Per-device table — NOT in CRR_TABLES" and startup line `[SCHEMA] ✓ chat_sessions (per-device, NOT CRR)`), corroborated by §30.1.1 and ADR-0052 D1 — and it could not be one: its partial unique index (`idx_sessions_active_per_surface`) fails cr-sqlite's CRR compatibility gate, which rejects any non-PK unique index (`is_table_compatible`, cr-sqlite `core/rs/core/src/tableinfo.rs:909-925` @ upstream main, fetched 2026-08-16). **Local-only, no sync surface:** the ADD COLUMN needs no `crsql_begin_alter`/`crsql_commit_alter` wrapping, the backfill UPDATE has no LWW or cross-device dimension, and no migration-timing window exists between devices — the treatment #2538's audit gave `ai_settings`. `chat_messages_local` (D4's stamp table) is likewise non-CRR (`migrations.rs:1453`; §24.8.1). Both memberships get the letter-of-record confirm against the `CRR_TABLES` array itself (`migrations.rs:32-57`) in the ADR-0063 D4 micro-fetch.

### D4 — Structural attribution: message agent = session agent; #2248 closed by elimination

`save_chat_message` stops reading `ai_agents WHERE is_main = 1` and stamps assistant messages from the binding: `SELECT agent_id FROM chat_sessions WHERE id = ?thread_id` (`thread_id` is already a required parameter since v72/ADR-0052).

Because the binding is immutable, save-time and dispatch-time agree **by construction** — the §24.8.2 divergence window (response completion → persistence) ceases to exist rather than being narrowed. #2248's stream-time-capture retrofit is unnecessary; it closes with a pointer here.

Preserved verbatim from ADR-0059 D5: assistant-only stamping; NULL means "written before attribution shipped," never "the main agent"; no backfill of message rows; per-device, non-CRR. A deactivated bound agent still stamps — that is correct provenance, and rename-relabels-history via the render join is unchanged (§24.8).

The stored column stays load-bearing (render reads it) — join-through-session at read was rejected because it would orphan the column into an ADR-0061 Decision 7 dark-config shape and silently re-attribute NULL-legacy rows, violating the preserved D5 semantics.

### D5 — Phase-1 dispatch semantics: out-of-class bound session is derived read-only, never re-bound

Phase 1 keeps the global provider live, so a bound session's agent can fall outside the active provider class (class switch away), or be deactivated (OC-side removal). Contract:

- **The state is derived, never stored.** Read-only-ness is computed per render from (`session.agent_id`'s class or `is_active`) vs the active provider class — nothing is written on the session (ADR-0061 D4's derived-not-stored precedent; no drift possible). Switching the provider class back re-enables the session with zero writes.
- **UI:** composer disabled with an inline notice naming the bound agent and why it is unavailable, plus a "Start new session" affordance (creates a session bound to the current designated default). History remains readable.
- **Backend:** `ai_chat_stream` against an out-of-class or inactive bound agent returns an explicit error naming the bound agent. **Never silent re-bind, never dispatch through a different agent.**

### D6 — `is_main` mutation stops affecting existing conversations

Changing `is_main` (manual "Set as Main," ADR-0059 D3 switch picker, OC re-import promotion) no longer changes who answers any existing session — bindings are immutable. Its remaining phase-1 effects: the default for new sessions and the sessionless background surfaces (D7). This is the precise supersession of ADR-0045 D1–D3's *usage* as chat-switch; the demote-then-promote transaction shape itself (D2) is untouched and continues to govern every flag change.

`pane-agent-switched` consequently narrows: session chat headers derive from the binding and do not listen for it; the event remains the refresh signal for default-agent display surfaces (§24.7B supersession in the spec pass).

### D7 — Ambient resolution rule: session agent where a session is in scope; designated default otherwise

One rule, applied per audited site (audit §2's five):

| Site | Phase-1 resolution |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| Message attribution (`commands.rs:9523`) | Session agent (D4). |
| Daily-log creation (`commands.rs:5567-5580`, inside `ai_chat_stream`) | Session agent — the log lands in the bound agent's workspace. |
| Memory router (`memory_router.rs:106`) | Session agent where the operation is message/thread-scoped (session derivable via `thread_id`); designated default otherwise. |
| Consolidator (`consolidator.rs:43`) | Designated default (background, no session). |
| Workspace-path resolution (`database.rs:424`) | Parameterized: session agent where the caller holds one; designated default otherwise. |

No new ambient `is_main` lookup may be added on a session-scoped path (INVARIANTS entry added in the same pass).

### D8 — Rides in phase 1

- **`restore_non_oc_main` removed:** command, `_inner`, registration (`lib.rs:1673`), and its tests (`commands.rs:10475-10556`, `:15012-15236`). Dead from the UI since ADR-0059 D3; a live registered command that mutates `is_main` via a retired selection rule is pure hazard (audit §2).
- **Provider snapshot fix (incidental, audit §1c):** `ai_chat_stream` acquires the config lock once and snapshots provider-derived state; the model-resolve read and the base_url/api_key extraction stop being two separate lock acquisitions.

### D9 — Session header shows the bound agent

Spec-level requirement; visual treatment is the Claude Design pass that follows this session.

## What phase 1 does NOT do

Binding is identity, not concurrency. `AiConfig` and `StreamManager` remain singletons; one stream at a time app-wide remains the phase-1 reality; two sessions bound to two in-class agents get correct identity, context, attribution, and memory routing — not simultaneous streams. Cross-class binding at dispatch, runtime pinning, and the singletons' demolition are ADR-0063.

## Constraint-collision enumeration (`chat_sessions` — this ADR defines its INSERT/UPDATE behavior)

| Constraint | Columns | Collision decision |
| ------------------------------------------------------------ | ---------------------- | ------------------------------------------------------------ |
| `PRIMARY KEY (id)` | `id` | Fresh UUIDv4 per insert; a collision is a UUID-reuse bug → error/rollback, never expected. The backfill UPDATE touches no key. |
| `idx_sessions_active_per_surface` — `UNIQUE(project_id) WHERE is_active = 1` | `project_id` (partial) | Cannot collide via this ADR: `agent_id` is not in the index; all four INSERT sites keep their existing active-flag semantics (born-active vs insert-inactive-then-promote); the backfill UPDATE sets only `agent_id`. Existing demote-then-promote switching is unchanged. |
| `idx_sessions_surface` | non-unique | Not a constraint; no decision required. |

`ai_agents` takes no INSERT/UPDATE behavior change in phase 1 — its enumeration lives in ADR-0063, which changes that table.

## Consequences

**Positive:** the attribution defect is eliminated structurally, not narrowed; #2248 closes without the cross-layer threading it scoped; "which agent is this conversation with" becomes a stored fact with a header, not an ambient inference; the five ambient sites get one uniform rule.

**Negative:** a class switch strands bound sessions read-only until switched back — the intended cost of never re-binding (mirrors ADR-0059's "intended cost of correctness"). The picker adds one decision to explicit session creation (default preselected keeps the fast path one click).

**Risks:** a future convenience "move this chat to another agent" feature would reintroduce re-binding; the INVARIANTS entry exists to make that a deliberate supersession, not a drive-by. The self-heal write in D3.4 is the single sanctioned post-INSERT write path and must not grow.

## Alternatives considered

**Stream-time capture (#2248 as scoped)** — rejected: threads agent identity through the streaming return path, captured-context carrier, callback layer, and save command to fix a window that immutable binding deletes outright.

**Join-through-session at read (no stamped column)** — rejected: orphans a live column (ADR-0061 D7 shape) and re-attributes NULL-legacy rows against preserved D5 semantics.

**Mutable binding with a re-bind command** — rejected by locked decision ("different agent = new session"); also reopens the attribution window D4 closes.

## References

- docs/audits/2026-08-16-per-session-binding-prespec.md @ 9c96a5fd — §1c (double-read race), §2 (five ambient sites, `restore_non_oc_main` state), §3a (schema), §3c (four INSERT sites), §3d (dispatch trace), §3g (derive-at-save quote)
- sessions.md §30.1–30.2, §30.7 (amended under this arc); agents.md §24.7, §24.7A/B, §24.8 (amended); §24.8.2 (superseded)
- ADR-0045 (D2 shape preserved), ADR-0052 (thread_id load-bearing), ADR-0059 (D5 semantics preserved; in-class gating unchanged), ADR-0061 (D4 derived-state precedent, D6 routing scope, D7 dark-config rule)
- #2522, #2248 (closed by elimination), #2536, #2538 (CRR-analysis rigor precedent)
- cr-sqlite upstream @ main (fetched 2026-08-16): `core/rs/core/src/tableinfo.rs:909-925` (`is_table_compatible` — non-PK unique indexes barred from CRRs)

---

## Amendment 2026-08-17 — Project creation binds by initiator

**Ruling (Matt, 2026-08-17, final).** D2 classified project-born-active as automatic and bound it to the designated default with no UI. **That row is superseded** — the original table above stands as the historical record. Project creation is not one path but two, split by initiator, and neither is silent:

| Path | Site | Binding |
| ----------------------------- | ----------------------------------------------- | ------------------------------------------------------------ |
| New Project (dialog) | `projects.rs:122` via `create_project_internal` | **Agent picker in the New Project dialog**, designated default preselected — the one-click fast path is preserved. In-class agents only in phase 1 (§24.7C), same rule as `create_session`'s picker. |
| `create_project` (agent tool) | same INSERT site, tool caller | **The acting agent** — the agent executing the tool, never the designated default. Resolved from the tool call's own session binding: the R1-ratified `session_id` threading through the tool loop (`commands.rs` sites 1608/2692/2312) makes the session — and via D1's immutable binding, the agent — resolvable at the call site. **No new plumbing.** |

**Mechanism.** `create_project_internal` gains a required `agent_id` parameter: wired from the dialog's picker on the UI path, and from the acting agent's session binding on the tool path. The born-active first session (§30.2.6 Eager) binds that value.

**The governing principle, stated three-way** (supersedes D2's two-way explicit/automatic framing; future creation paths classify by initiator under it without a new ruling):

- **User-initiated creation shows the binding.** New Session and New Project are both explicit acts; both get the picker, default preselected. A silently default-bound first session behind a new project would be wrong roughly half the time for anyone running two runtimes — an unwanted hidden session the user never chose.
- **Agent-initiated creation binds the acting agent.** The user asked *that* agent to set the project up; continuity keeps the first session with the agent they addressed.
- **Only system-initiated creation binds the designated default silently.** `get_active_session` lazy-create and `hide_session`'s last-visible fallback — the paths where no UI can exist and no agent is acting.

**Untouched:** binding immutability (D1), structural attribution (D4), migration/backfill and the self-heal contract (D3), the derived read-only contract (D5), `create_session`’s row, and both system paths. Picker visuals are the Design pass’s (one mock: the New Project dialog agent row); this amendment is behavioral only.

---

## Amendment — 2026-08-23: D5 scope clarification (append-in ≠ dispatch-out)

**Context:** §30.11 (external message ingestion, #2410) introduces a path by which an agent’s messages enter a session transcript without dispatch. D5 (“out-of-class bound session is derived read-only, never re-bound”) governs dispatch refusal for locked/out-of-class agents but is silent on whether externally-produced content from that agent may still land in the session.

**Decision:** **D5 governs `ai_chat_stream` dispatch only.** External message ingestion (§30.11) is not dispatch — a session bound to a locked or out-of-class agent still accepts inserted messages from that agent via the ingestion path. Only the dispatch refusal in D5 stands. Rationale: delivery is transcript history — a record of what the agent already said on its own runtime — not an invocation of the agent by Pane. Refusing delivery would silently discard real agent output; refusing dispatch remains correct because dispatch is Pane *acting through* the agent.

**Supersedes:** nothing — clarifies D5’s scope. D5’s text is unchanged.
