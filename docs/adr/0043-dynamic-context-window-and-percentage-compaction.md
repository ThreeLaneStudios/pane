# ADR-0043: Dynamic Context Window Sourcing + Percentage-Based Compaction

*Line citations predate the #3097 module split.*

**Status:** Accepted
**Date:** 2026-06-08
**Issue:** #1324 (launch-blocking)
**Supersedes:** none — confirmed against all ADRs cited in spec+INVARIANTS (0011–0042); none governs budget/compaction.
**Spec:** `docs/spec/ai-integration.md` §27.9–§27.11

## Context

Context-window size was a hardcoded name→size table (`get_context_window`, `context_budget.rs:158`): Claude→200k, GPT→128k, else 128k. Nothing came from the provider. The window is wrong the moment a model ships a different size, and Ollama models were universally mis-sized to 128k. The budget compaction trigger (`commands.rs:6472`, `total_messages > messages_included + 12`) is derived from that table's output, so it inherited the staleness. Separately, the same model is sold at different windows at different price tiers — Pane had no way to know which the user pays for.

## Decision

1. **Source the window dynamically; `get_context_window` becomes a resolver, not a table.** Precedence: OC-gateway emission → provider API (OpenRouter `context_length`, Ollama `/api/show`) → user override → static seed → 128k. The function remains the single authoritative window entry point (G5 anchor); the table survives only as a last-resort, "estimated"-labeled, logged, user-overridable seed.
2. **Compaction's budget trigger fires on `tokens_demanded / window ≥ 0.80`**, measured on the full untrimmed estimate — not post-trim usage (≈ window by construction). Replaces the `+12` message-count trigger. The latency trigger is untouched.
3. **Non-OC users set the window** (free numeric, floor 50k, clamped to model max when known). OC users do not — OC sets it.
4. **Internal field name is `context_length`** (matches OpenRouter/Ollama wire); the gateway's field is mapped at the deserialize boundary.

## Rationale

- **Dynamic window** kills the stale-table failure class and is launch-blocking.
- **Percentage trigger** is justified solely by *proactive vs reactive*: the `+12` trigger only fires after the budget manager has begun silently dropping messages, whereas percentage-of-demand fires while everything still fits and summarizes before loss. It is **not** justified by "scales with window" — the `+12` trigger already auto-adapts via `messages_included`. This rationale was tested in-session and accepted on the silent-drop ground.
- **Refactor, not delete** (`get_context_window`): G5 treats `context_budget.rs` as the single authoritative window source; deleting the function removes G5's anchor.

## Alternatives considered

- **Keep `+12`, only make the window dynamic.** Rejected: leaves the silent-drop gap. But note the window change alone *does* fix window-awareness — the trigger switch is a separable, smaller win, sequenced behind the `tokens_demanded` figure (#1324-D).
- **Select-of-known-tiers picker** instead of free input. Rejected: reintroduces a maintained per-model table — the hardcode being removed.
- **Per-model override key.** Deferred: global `context_window_override` + auto-clamp covers model-switching when the max is known; the unknown-max + model-switch edge is a post-launch follow-up.

## Consequences

- New `BudgetResult.tokens_demanded` field; Health dashboard surfaces it.
- New keys `context_window_override`, `compaction_trigger_pct`.
- New dependency on the gateway emitting a window (#1324-F); confirmed NOT emitted today (per 2026-06-09 OC dist inspection); until landed OC falls to seed (200k), no regression.
- G5 flips to `fail` once the resolver lands and the only window literals are the seeds inside `context_budget.rs` (#1324-G).

## Migration

None. No current users; no upgrade path to protect.
