# ADR-0059: Provider-Scoped Agent Usability and Local Agent Creation

**Status:** Proposed
**Date:** 2026-07-24
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** ADR-0045 D4, D5 (partial supersession — D1, D2, D3 preserved and reaffirmed)
**Supplements:** agents.md §24.2, §24.5.5, §24.6, §24.7A, §24.7B, §24.7C, §24.8, §24.10;
 sync.md CRR table list; ai-integration.md §27
**Related:** #2030, #2143, #2012 (framing superseded), #1492/#1527 (auto-restore partially retired),
 #956 (OC-authoritative origin), #1481 (slug-collision precedent), ADR-0022 (per-device state precedent)

## Context

ADR-0045 D4 fixed the stranded-`is_main` bug (#1492) by restoring "the unique non-OC agent" on a
provider switch away from OpenClaw. Its uniqueness claim rested on three premises, all stated in
D4 and all true at the time: onboarding creates exactly one `oc_agent_id = NULL` agent; v1 has no
"Add Agent" (§24.6); non-OC ↔ non-OC switches create no agents.

#2030 falsifies all three. Local agent creation makes the non-OC set arbitrarily large, so "the
unique non-OC agent" no longer identifies a row.

Separately, today an OC-onboarded customer who switches to an API provider keeps their OC agents
and animates them with the API model. #2012 observed this asymmetry and asked whether it was
intended. It was never ratified; this ADR closes it by removing it rather than blessing it. An
agent's identity files, memory, and provenance belong to the runtime that created them. Borrowing
them across the boundary produces an agent that is neither the OC agent the customer knows nor a
clean local one, and silently mismatches identity and memory files against a model that never
wrote them.

### What if we didn't decide

- `restore_non_oc_main` picks arbitrarily among N local agents, or picks the wrong one silently.
- Customers testing providers for cost or fallback keep landing in a state where the agent they
  see is not the agent they configured.
- #2012 stays open as a "known confusing behavior" through launch.

## Decision

### D1 — Hard provider separation, discriminated by `oc_agent_id` nullity

An agent is usable only under its own provider class. OC-imported agents
(`oc_agent_id IS NOT NULL`) are usable only while `provider = openclaw`; locally-created agents
(`oc_agent_id IS NULL`) are usable only under an API provider (OpenRouter / Anthropic / Ollama).
Agents outside the active class are **rendered disabled — visible, never hidden**.

The discriminator is `oc_agent_id` nullity, **not `source`**. `source` carries four values
(`import`, `onboarding`, `ai_created`, `recovered`) that do not partition into two groups: a
`recovered` row can be either an OC workspace folder or a local agent, so grouping on `source`
misfiles recovered agents.

`oc_agent_id` is additionally the right discriminator because it is **constraint-backed**:
`idx_ai_agents_oc_id_unique` (`migrations.rs:864`) guarantees no two rows share an OC identity.
The nullity test therefore partitions the table cleanly and unambiguously, and the same constraint
is what makes §24.5.5's overwrite-in-place match on `oc_agent_id` resolve to exactly one row.
Locals are unaffected: they write NULL, and SQLite treats NULLs as distinct under a unique index,
so any number of local agents coexist.

### D2 — `is_main` stays a single global flag, re-targeted on every switch

ADR-0045 D1 and D2 are preserved verbatim: one global `is_main`, changed only by
demote-then-promote inside a single transaction. What changes is the *selection rule*, now
provider-aware — `is_main` must always point at an agent usable under the active provider, and no
switch path may leave it otherwise.

No provider-scoped index (`UNIQUE(is_main, provider)`), no `last_active_at` column. §24.11 still
reserves `idx_ai_agents_main` for future `owner_id` scoping.

### D3 — Switch-path contract

| Transition | Behavior |
|---|---|
| → API provider, **zero** local agents | Forced create flow (D4). No fallback into OC agents. |
| → API provider, **≥1** local agent | Picker over local agents, "create new agent" always offered. Default is the most recently created usable agent (`created_at DESC`); no persisted per-provider memory. |
| → OpenClaw | Unchanged. Import + OC-authoritative main selection (§24.5.5, ADR-0045 D3, #956). |

This **retires the auto-restore behavior of ADR-0045 D4** on the →API path. The two must not
coexist: §24.5.5's reverse-switch paragraph is marked superseded in the same PR, and the code PR
retires the `restore_non_oc_main` call rather than extending it.

### D4 — Create flow: name-commit is the transaction boundary

The agent row is INSERTed and promoted to `is_main` in **one transaction at name-commit** — the
moment the customer confirms the agent name — not at flow start and not at flow end. Seeding runs
afterward, against a live agent, which is what makes "just start chatting" possible.

There is never a no-main interim. A zero-`is_main` window would break §24.7 context loading
(`WHERE is_main = 1 AND is_active = 1`) and the greeting path, which reads
`ai_agents.name WHERE is_main = 1`.

**Sequencing rule:** the provider switch does not commit until name-commit. Order is: provider
selected → key/connectivity validated → zero-locals detected → create flow → at name-commit the
agent row and the provider write commit together → `pane-agent-switched` dispatches (§24.7B).
Committing the switch first would leave the app on an API provider with `is_main` on an OC agent —
a state D1 declares invalid — with the §22C.11A.4 key wipe having already destroyed the OC
credentials the customer would need to go back.

**Cancellation.** Before name-commit: clean no-op — no agent row, no provider write, no key wipe.
After name-commit: the agent exists and the switch is committed; incomplete seeding is not an error.

Locally-created agents carry `source = 'ai_created'`. No new `source` value is introduced.

### D5 — Per-message agent attribution

Hard separation makes chat history ambiguous: a thread can contain messages written by an agent
that is not usable under the active provider. Rendering them under the current agent misattributes
them.

`chat_messages_local` gains a nullable `agent_id`, stamped at write time on assistant messages and
rendered per message (§24.8). **`NULL` means "written before attribution shipped" — it does not
mean "the main agent."** The prior §24.8 text defined NULL as "main agent"; under this ADR that
semantic would silently re-attribute all historical messages every time the customer switches
providers, which is precisely the failure this column exists to prevent.

Pre-existing rows migrate to `NULL` and render under the current agent — the stated and accepted
limitation. They are **not backfilled**: which agent wrote a historical message is unknowable, and
a guess would be fabricated provenance in the one field whose purpose is provenance.

`chat_messages_local` is **not a CRR table** (`migrations.rs:1370`, "Per-device table"), so the
attribution is per-device and does not sync — consistent with ADR-0022's per-device precedent and
with v1's single-machine posture (onboarding §6: multi-Pane installs are not supported in v1).

### D6 — No deletion or archiving in v1

The agent list only grows; system files persist on disk. This continues §24.5.4's posture
(ADR-0007): Pane does not delete files that were once the customer's. OC-side removals deactivate
(`is_active = 0`), they do not delete.

## Consequences

### Positive
- "The unique non-OC agent" is replaced by explicit selection — no ambiguity as the roster grows.
- One constraint-backed discriminator serves usability, grouping, and selection.
- #2012 closes by elimination rather than ratification; both switch directions are symmetric.
- ADR-0045 D1/D2/D3 survive untouched; the demote-then-promote shape is unchanged.

### Negative
- A provider switch is no longer silent on the →API path: it can require a decision (picker) or a
  whole flow (forced create). That is the intended cost of correctness.
- `restore_non_oc_main` changes role rather than being extended — a shipped code path is partly
  retired five weeks after landing.
- D5 is the first schema change under the post-tester migration policy, and it lands on the
  chat-history table — the highest-consequence table to get a migration wrong on.

### Risks
- **Both selection paths firing.** If the code PR adds the picker without retiring auto-restore, a
  switch could restore one agent and then present a picker defaulting to another. The spec marks
  §24.5.5's paragraph superseded specifically to prevent this; the code audit must confirm
  `restore_non_oc_main` is no longer invoked on the →API path.
- **Migration on chat history.** The `agent_id` add must be purely additive — no DROP, no recreate,
  no data movement. Existing messages must survive unchanged.
- **Sequencing regression.** If a later refactor moves the provider commit ahead of name-commit,
  the invalid state returns silently. Worth a test asserting no committed provider change exists
  without a usable `is_main`.

## Alternatives Considered

**Ratify the asymmetry (#2012's "onboarding-anchored identity").** *Rejected:* it is the behavior
customers reported as confusing, and it mismatches identity/memory files against a model that
never wrote them.

**Provider-scoped `is_main`** (`UNIQUE(is_main, provider)`). *Rejected* for ADR-0045 D1's original
reasons, plus §24.11 reserves that index for `owner_id`.

**`last_active_at` column for per-provider main memory.** *Rejected:* the picker is the selection
mechanism, so no persisted memory is load-bearing; `created_at` gives an adequate default at zero
schema cost. Revisit post-launch if the default proves wrong.

**Hide unusable agents instead of disabling them.** *Rejected:* a roster that silently shrinks on a
provider switch is more alarming than a disabled row, and it hides the reason.

**Skip per-message attribution; accept thread-level ambiguity.** *Rejected* per Matt's ruling — under
hard separation the misattribution is visible on the busiest surface in the app.

## Supplements / Amendments

### 2026-08-03 (#2392): voluntary in-place creation while OpenClaw is active

D3's switch-path contract enumerates create-agent behavior when the provider is *changing*. It did
not address the voluntary "+ New agent" click in Settings → Agents while `provider = openclaw` and
no switch is in progress. Completing the matrix:

- **While OpenClaw is active, voluntary local creation is disabled** — the "+ New agent" control is
  shown disabled (not hidden) with an explanatory note (agents.md §24.6). It is not a provider
  picker (rejected: forces API-provider validation onto a create-agent action that has not asked to
  switch), and it is not a name-first flow that lands an immediately-disabled agent (rejected:
  produces a dead-end agent, `oc_agent_id IS NULL` and unusable per D1/§24.7C the moment it exists).
- **To create a local agent, the customer switches to an API provider** — at which point the
  existing D3 paths apply (forced create on zero locals; picker with "create new" on ≥1).

This is a direct consequence of D1 (usability by provider class), not a new principle. It is recorded
here because D3 is where the create-agent-per-provider-state matrix lives, and it was incomplete.

## References

- §24.2 `ai_agents` schema (five indexes, `migrations.rs:864`); §24.5.5, §24.6, §24.7A/B/C, §24.8, §24.10
- ADR-0045 (D1/D2/D3 preserved; D4/D5 superseded here); ADR-0022 (per-device precedent)
- INVARIANTS.md "Agent Identity"
- #2030 (implementation consumes this via `spec-covered-by:`), #2143, #2012 (close with pointer),
  #2206 (ADR index hygiene), #2207 (migration-policy mechanization)
