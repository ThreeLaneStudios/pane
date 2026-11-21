# ADR-0045: Main-Agent (`is_main`) Selection Contract

**Status:** Proposed
**Date:** 2026-06-19
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** §24.5.5 (provider-switch re-import), §24.7A (main-agent selection),
 the #956 decision "OC gateway authoritative on who is main" (Matt, 2026-06-15)
**Related:** #1492 (this), #956 (one-directional hijack origin), #1527 (update-path demotion gap — must land with #1492), #1480/#1481 (slug-collision cluster),
 §24.2 (`ai_agents` schema), §24.6 (Settings → Agents UI, NOT BUILT)

## Context

`is_main` is a single global flag: the partial unique index
`idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1` (§24.2) permits at most one
`is_main = 1` row. Onboarding on any non-OC provider (OpenRouter, Anthropic, Ollama) creates
exactly one agent via `ensureMainAgent` with `oc_agent_id = NULL` and `is_main = 1`.

Switching **to** OpenClaw runs `sync_re_import`, which (per #956) demotes the existing
`is_main = 1` row and promotes the imported OC main agent — "OC gateway authoritative on who
is main." Switching **back** to a non-OC provider runs no agent re-import (§24.5.5 fires only on
a change *to* OpenClaw; §22C.11A.4 wipe scope clears AI-config keys only, never agent rows).

Result: after a non-OC → OC → non-OC round-trip, `is_main` is stranded on the OC agent with no
path back to the user's original main. This is the #1492 gap. It is **additive to #956**, not a
reversal: OC remains authoritative *while OC is the active provider*; this contract governs only
what happens when OC is no longer active.

## Decision

### D1 — `is_main` stays a single global flag

No data-model change. Provider-scoping `is_main` (e.g. `UNIQUE(is_main, provider)`) is rejected:
it requires a migration on a launch-blocking path, and §24.11 already reserves
`idx_ai_agents_main` for future `owner_id` scoping. The problem is not "two mains at once" — it is
"no path to move the one main." A single flag plus a deterministic selection rule solves it.

### D2 — Two operations, one transaction shape

`is_main` changes only via **demote-then-promote within a single SQL transaction**: clear the
current `is_main = 1` row, set the new one, commit atomically. Never two independent UPDATEs
(a crash between them violates the index or leaves zero mains). This is the shape #956 already
uses for the →OC direction (T13).

Two callers exercise it:

- **Auto-restore (reverse switch, launch-blocking).** On a provider change *away* from OpenClaw,
  Pane re-promotes the prior non-OC main. No UI required. Closes #1492.
- **Manual Set-as-Main (durable mechanism).** A "Set as Main" control in Settings → Agents
  (§24.7A, surfaced when §24.6 UI lands) runs the same transaction. See D5 for v1 scope.

### D3 — OC authoritative while active (unchanged from #956)

While `provider = openclaw`, the gateway dictates `is_main`; switching to OC re-claims it every
time. A user's off-OC main choice does **not** survive a later switch to OC — OC re-claims, and on
the next switch back, auto-restore (D4) re-applies the prior non-OC main.

### D4 — Restore target: the unique non-OC agent, identified by `oc_agent_id IS NULL`

The agent to restore on switch-back is the row matching `oc_agent_id IS NULL AND is_active = 1`.

This is the former non-OC main **by construction**, and is unique in v1:
- onboarding creates exactly one `oc_agent_id = NULL` agent;
- v1 has no "Add Agent" (§24.6) — every other agent is OC-imported and carries an `oc_agent_id`;
- non-OC ↔ non-OC switches create no agents.

No new column (`demoted_from_main_at` or similar) is introduced: with a singleton candidate set,
a stamp disambiguates nothing it does not already. (Considered and rejected — see D5 / V2 note.)

**Enumerated outcomes on switch away from OC:**

| State | Action |
|---|---|
| Exactly one `oc_agent_id IS NULL AND is_active = 1` row exists | Demote current `is_main`, promote that row. One transaction. |
| No such row (OC-native onboarding — all agents OC-sourced) | No-op. `is_main` stays on its current (OC) row. Correct: there is no prior non-OC main to restore. |
| Restore transaction fails (DB/file error) | Leave `is_main` unchanged; surface error. Never a partial flip. |

### D5 — Manual selection is spec'd; its persisted state is V2-gated

D2's manual "Set as Main" is the durable mechanism (decision 2). But in v1 the off-OC candidate
set is size 1, so there is nothing to choose and nothing to persist — the off-OC main is
deterministically the `oc_agent_id IS NULL` agent (D4). A persisted user-selected-main signal
becomes necessary only when V2 (AI-assisted agent creation) can produce ≥2 non-OC agents. At that
point, introduce the persisted signal — and only then does a marker column earn its place.

## Dependencies

**The "single `is_main`" guarantee (D1; INVARIANTS.md, "Agent Identity") is not yet held by code.** Reed re-audit #1527
found that the #956 `is_main` demotion fires only on the **fresh-import** path of
`sync_re_import`, not the **overwrite-in-place UPDATE** path. Today the code can therefore leave
two `is_main = 1` rows — the invariant this ADR declares is an as-designed guarantee, not a
current runtime property.

Consequences for this contract:
- **Auto-restore (D4) is unsafe until #1527 lands.** If a stranded second `is_main = 1` row
  already exists, promoting the restored non-OC main on top of it produces *three* — or, depending
  on which row the demote targets, leaves the strand untouched. Auto-restore assumes it starts
  from exactly one main; that assumption is #1527's to establish.
- **Mitigation (Reed's, adopted):** add a `SELECT COUNT(*) WHERE is_main = 1` assertion at the top
  of `sync_re_import` and a demotion guard on the overwrite-in-place UPDATE path, so every
  re-import path normalizes to one main.

**Sequencing:** #1492 and #1527 land **together**. #1492's demote-then-promote logic is correct on
its own, but its correctness *in production* depends on the table holding the single-main
invariant first. Do not merge auto-restore against a code base that can still strand a second main.

## Consequences

- **Positive:** #1492 closed with zero schema change. One transaction shape governs every
  `is_main` change. #956 preserved intact. Restore is deterministic, not heuristic.
- **Negative:** "Set as Main" ships as an inert-in-v1 control (its selection value is V2). Off-OC
  users with an OC-native install get no restore — acceptable, as they never had a non-OC main.
- **Invariant added:** see INVARIANTS.md, "Agent Identity". Not yet held by code — depends on #1527 (see Dependencies).

## References

- §24.2 `ai_agents` schema; `idx_ai_agents_main` partial unique index
- §24.5.5 provider-switch re-import; #956 demotion (CHANGELOG, T13)
- §24.7A main-agent selection (added by this ADR)
- #1492 (implementation consumes this ADR via `spec-covered-by:`); #1527 (couple at code-PR time)
