# ADR-0042: LWW Tie-Break Resolves to Pane (Source of Truth)

**Status:** Accepted
**Date:** 2026-06-02
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (first ADR on LWW tie-break direction; supersedes only the inherited spec line in §22C.8.2 and the inherited bridge behavior referenced in ADR-0015's Risks section)
**Supplements:** ADR-0012 (Primary-Replica model — Pane is PRIMARY, OC is REPLICA), ADR-0015 (Gateway↔OC transport — references inherited "LWW with gateway tie-break" in Risks as bridge behavior carried forward), ADR-0040 (Gateway workspace resolution — treats §22C.8.2 as LWW baseline for its stub-migration override), ADR-0041 (Sync scope ⊊ persistence scope — flags #1299 as launch-blocking)
**Related:** `docs/spec/sync.md` §22C.8.2 (rule), §22C.11.2a (cross-ref), §22C.15 (test assertion), `docs/spec/README.md` §0.3.3 (Primary-Replica Principle), #1299 (the bug this ADR closes)

## Context

`docs/spec/sync.md` §22C.8.2 currently codifies LWW conflict resolution with this rule:

> **Timestamp-based LWW with gateway tie-break.** Behavior identical to current bridge:
> 1. On pull, for each pending change, compare local file `modified_at` vs remote `queued_at`.
> 2. Newer timestamp wins.
> 3. Ties resolve to the gateway (deterministic; avoids ping-pong).
> 4. Content-identity check: if local content == remote content after the timestamp says "ingest," skip the write.

Step 3 — ties resolve to the gateway — contradicts the Primary-Replica model in `docs/spec/README.md` §0.3.3: **Pane is PRIMARY, OC is REPLICA. Pane owns the database and flat files. Push direction: Pane → OpenClaw — always.** Under the model, when timestamps tie, Pane's version is by definition the canonical one; resolving to the gateway means accepting the replica's copy over the source-of-truth.

The provenance audit (#1299) established that gateway-wins was never a decision. It entered the codebase as a single line in commit `57dab52da`, framed as "behavior identical to current bridge." No ADR was filed. No rationale exists anywhere — not in §22C.8.2's surrounding prose, not in ADR-0012 (Primary-Replica), not in ADR-0014 (bridge vs Supabase split), not in ADR-0015 (transport). The "deterministic; avoids ping-pong" parenthetical in §22C.8.2 is true of either direction — a deterministic rule resolving to Pane is equally ping-pong-safe. The choice of direction was inherited, not deliberated.

ADR-0015's Risks section references the inherited rule: "the version that lands is the right one (timestamp comparison + LWW with gateway tie-break + content-identity skip, before the write). The same race exists today; the bridge plugin's queue handled it via the same conflict-resolution logic that the gateway now reuses through the shared module." That reference describes the inherited behavior accurately for the moment it was written, and remains correct as the historical record of the bridge-era rule. It is preserved per playbook §2 (ADRs immutable); this ADR carries the corrected rule forward without editing ADR-0015's body.

ADR-0041 flagged #1299 explicitly as launch-blocking: the LWW tie-break inversion contradicts the Pane-PRIMARY model and must be fixed before launch. This ADR is the fix at the decision layer; the spec amendments and Stia's `conflict_resolution.rs` flip are the fix at the spec and code layers.

The bug is launch-blocking because the failure mode is silent. A user editing a system file on the Pane machine at the same wall-clock second as OC writing the equivalent file on the OC machine loses their edit — gateway-wins discards Pane's version. With ms-granularity timestamps and the cadence at which OC's autonomous agents write to system files (memory consolidation, daily logs, heartbeat updates), exact ties are uncommon but not rare; the failure mode happens often enough that "silent data loss on the user's edits" is a real risk and frequent enough that production logs would surface it after launch.

What if we didn't decide:

- **The current rule ships.** Pane-PRIMARY remains the documented model in §0.3.3 and ADR-0012, but the actual conflict-resolution code resolves to the replica. The contradiction stays in production. Silent data loss on ties.
- **The fix lands without an ADR.** Stia flips the equal-timestamp tie-break branch in `conflict_resolution.rs` (currently ~68-72), the spec gets a one-line edit. No durable record of *why* — the next engineer reading §22C.8.2 finds "Pane wins" but no explanation, and if a future "performance optimization" or "simplification" proposes resolving to the gateway again (perhaps to match a future OC behavior, or "for symmetry with X"), there's no scar tissue to flag it as architecturally wrong.
- **ADR-0015 stays the only documented framing of the rule.** ADR-0015's Risks reference to "LWW with gateway tie-break" reads as the canonical rule rather than the inherited-and-since-corrected one. Future readers loading ADR-0015 without ADR-0041 or this ADR in context get the wrong impression.

## Decision

**On a timestamp tie in LWW conflict resolution, Pane wins.**

Specifically, the §22C.8.2 rule becomes:

1. On pull, for each pending change, compare local file `modified_at` vs remote `queued_at`. (Unchanged.)
2. Newer timestamp wins. (Unchanged.)
3. **Ties resolve to Pane (deterministic; honors Pane-PRIMARY / OC-REPLICA per ADR-0012 and §0.3.3).** This is the change.
4. Content-identity check: if local content == remote content after the timestamp says "ingest," skip the write. (Unchanged.)

Concretely:

1. **Direction is Pane-wins, not gateway-wins.** When `local_modified == remote_timestamp` to the millisecond, the local file is preserved and the remote change is discarded. In `conflict_resolution.rs`, the tie branch returns `Resolution::KeepLocal` (or equivalent), not `Resolution::AcceptRemote`.

2. **Determinism is preserved.** Both gateway-wins and Pane-wins are deterministic rules; both avoid ping-pong. The "deterministic; avoids ping-pong" rationale in the original §22C.8.2 parenthetical applies equally to Pane-wins. The flip changes direction, not determinism.

3. **Content-identity skip is unaffected.** Step 4's check runs after the tie is resolved in Pane's favor. If local content matches remote (regardless of who "won"), no write happens — this avoids gratuitous filesystem churn and FTS5 invalidation. The flip affects only what happens when timestamps tie *and* content differs.

4. **Three spec sites are amended, content-anchored:**
 - §22C.8.2 step 3: "Ties resolve to the gateway" → "Ties resolve to Pane."
 - §22C.11.2a cross-ref: "LWW with gateway tie-break" → "LWW with Pane tie-break."
 - §22C.15 test assertion (category 2, conflict resolution tests): "Tie-case asserts gateway wins" → "Tie-case asserts Pane wins."

5. **ADR-0015 is not amended.** Per playbook §2, accepted ADRs are immutable. ADR-0015's Risks section reference to "LWW with gateway tie-break" remains the accurate historical record of the inherited bridge behavior; this ADR carries the corrected rule forward. A future reader of ADR-0015 follows the supersession chain (ADR-0015 → this ADR via Supplements) to find the current rule.

6. **ADR-0040 is not amended.** ADR-0040's stub-migration override is independent of tie-break direction — the override discards LWW *entirely* for the stub case, not just the tie sub-case. The override behavior is identical whether tie-break is gateway-wins or Pane-wins.

## Rationale

### Why Pane wins

Pane is PRIMARY per ADR-0012 and §0.3.3. The replica converging to the primary's state is the correct steady state, not the inverse. When timestamps tie, "the version that should win" is structurally Pane's — there is no architectural argument for OC's version to win in a tie. The previous rule's only justification was "behavior identical to current bridge," which was the inherited bridge behavior, not a decision against the alternative.

### Why determinism wasn't the real argument for gateway-wins

The original §22C.8.2 parenthetical reads "deterministic; avoids ping-pong." Both candidate rules (gateway-wins, Pane-wins) are deterministic. Both avoid ping-pong by the same mechanism: a fixed tie-breaker prevents A→B→A→B oscillation when both sides write at the same instant. The parenthetical was true of the inherited rule but does not distinguish it from the alternative. The actual argument needed to be "given a tie, who wins" — and that argument has only one answer under Pane-PRIMARY.

### Why this is an ADR (not just a one-line spec amendment)

The original rule shipped via a one-line code-and-spec change with no decision artifact. Flipping it via a one-line edit in reverse would repeat the same gap — the corrected rule lands in spec, no rationale anywhere, the next engineer who finds it has no record of *why*. The ADR holds the why. Specifically: a future "performance optimization," "symmetry-with-OC" change, or "we should match $X's behavior" proposal that re-introduces gateway-wins would have to overturn this ADR explicitly rather than slide in under the radar. Same shape as ADRs 0040 and 0041 — the ADR carries principle so the spec can hold mechanics without bearing the full rationale.

### Why ADR-0015 is not amended

Playbook §2 makes ADRs immutable for audit-traceability reasons. ADR-0015's reference to "LWW with gateway tie-break" was accurate at the time it was written — that *was* the inherited rule. Editing the body to retrofit the corrected rule would erase the historical record of how the bug entered the codebase, which is exactly the kind of evidence #1299's provenance audit needed to find. The supersession-by-supplement pattern (this ADR Supplements ADR-0015) handles the correction without violating immutability. ADR-0019's clarification of ADR-0006 (PR #690 incident) is the precedent.

### Why the spec-line provenance matters

ADR-0041's "implementation-transition-gap" pattern (named in its Follow-ups) is the same shape that produced #1299. A line that entered the codebase framed as "behavior identical to X" — without an ADR, without a rationale — survived three audits and a year of dogfood because nobody questioned the inherited frame. The corrective discipline is: when spec text reads "behavior identical to [retired system]," that phrase is a flag for an absent decision, not a substitute for one. Filed as a follow-up sweep below.

## Consequences

### Positive

- **#1299 closes.** Tie-case data loss on user edits stops. Launch-blocking item from ADR-0041 cleared.
- **§22C.8.2 aligns with ADR-0012 and §0.3.3.** Spec internal consistency restored — the Primary-Replica model and the LWW rule no longer contradict each other.
- **Durable record for the decision.** Future readers of §22C.8.2 find an ADR explaining the direction; a future "symmetry" or "performance" proposal that would re-introduce gateway-wins is structurally harder to land silently.
- **No INVARIANT change required.** Tie-break direction is a sub-rule of §22C.8.2's LWW behavior; no separate INVARIANT exists today. The flip lives in spec text, not in INVARIANTS, which keeps the change small.

### Negative

- **The bridge plugin's behavior is no longer "identical" to the gateway's.** §22B.3.5 (legacy bridge spec) implicitly carried the same gateway-wins rule. Bridge is on the 2026-05-07 audit's RETIRED list and §22B is no longer the active spec, so the divergence is academic. Flagged in References so a future bridge-retirement spec touch reflects the corrected rule if it ever revisits §22B.3.5's body.
- **One more ADR in the index.** Cheap; ADRs are short and the bar for cross-referencing is exactly the structural-decision threshold this ADR meets.
- **Stia's implementation PR is launch-gating until #1299 closes.** This ADR doesn't ship the code fix; merging it doesn't close the bug. The bug closes after the code PR + hardware verification on a real two-machine setup.

### Risks

- **A future change re-introduces gateway-wins under a different name.** A "performance optimization" that resolves ties to whichever side is closer to disk, or "symmetry with OC's internal conflict resolution," or "match X's behavior" could effectively be gateway-wins under another framing. Mitigation: this ADR's Rationale names the architectural argument (Pane is PRIMARY); any re-introduction must overturn the Pane-PRIMARY framing, not just propose a new tie-break rule. The ADR is the audit checkpoint.
- **Ms-granularity timestamps make ties uncommon but not unprecedented.** Two writes at the same wall-clock millisecond do occur — same agent's heartbeat at second boundaries, two machines with synchronized clocks editing the same file simultaneously. The flip changes which writes are preserved in those cases; pre-launch dogfood is unlikely to surface a noticeable change because the population is small, but the failure mode the flip prevents (Pane-edit silently overwritten) was always silent regardless.
- **Hardware verification post-implementation must include the tie case.** A flip that doesn't actually flip the runtime behavior (because the test was changed but the code wasn't, or vice versa) is the kind of thing the audit cadence catches. The implementation PR includes the unit test flip and integration test coverage per §22C.15.

## Alternatives Considered

**Keep gateway-wins; document the rationale post-hoc.** Find an architectural reason gateway-wins is defensible and write it down. **Rejected** because no architectural reason exists. The Primary-Replica model is unambiguous; gateway-wins contradicts it. Documenting a contradictory rule as "intended" would be revisionist.

**Resolve ties based on content-identity skip alone.** Skip the write if content matches; if it differs, raise an error or surface a conflict to the user. **Rejected** because (a) user-surfaced conflicts contradict §22B.3.5's original framing of "no user dialogs, all reconciliation automatic" — a principle that carried forward and is still load-bearing; (b) surfacing conflicts for ms-granularity ties is operationally noisy and gives no meaningful choice (the user can't tell which version is "theirs"); (c) the Primary-Replica model already provides the right answer (Pane wins) without needing user input.

**Add a configurable tie-break direction in user settings.** Let the user choose. **Rejected** as overengineering. The right answer is determined by architectural model, not user preference. Configurable tie-break would create a per-user split between two rules with no migration path for the wrong-direction config to ever be flipped back.

**Defer the fix; document the divergence between spec and Primary-Replica.** Ship with gateway-wins still in code, mark §22C.8.2 with a "known discrepancy" note. **Rejected** because the discrepancy is silent data loss in production, and ADR-0041 already classified #1299 as launch-blocking. Deferring violates the launch-blocking commitment without an architectural reason.

**Make the tie-break depend on which side was last-known-good (e.g., last successful sync).** A stateful tie-break informed by sync history. **Rejected** because (a) it's more complex than the problem warrants — ties are rare, a simple rule suffices; (b) stateful tie-breaks introduce their own failure modes (history corruption, history rollback); (c) Pane-PRIMARY is already a global stateful "rule" — there's no need for a parallel state machine.

## References

- Bug: #1299 — LWW tie-break inverted for PRIMARY model (launch-blocking, flagged in ADR-0041)
- Provenance: commit `57dab52da` — the original "behavior identical to current bridge" framing that introduced gateway-wins without a decision artifact
- Code: the equal-timestamp tie-break branch in `src-tauri/src/conflict_resolution.rs` (currently ~68-72) — returns `Resolution::AcceptRemote` on tie (flipped to `KeepLocal` or equivalent in Stia's follow-up PR)
- Spec sites amended by this ADR: `docs/spec/sync.md` §22C.8.2 (rule), §22C.11.2a (cross-ref), §22C.15 (test assertion category 2)
- ADR-0012 — Pane Gateway as primary sync transport (Pane-PRIMARY / OC-REPLICA model; this ADR honors it)
- ADR-0015 — Gateway↔OC transport (Risks section references inherited "LWW with gateway tie-break" — preserved as historical record per playbook §2)
- ADR-0040 — Gateway workspace resolution (stub-migration LWW override; unaffected by this ADR because the override is independent of tie-break direction)
- ADR-0041 — Sync scope ⊊ persistence scope (flagged #1299 as launch-blocking)
- `docs/spec/README.md` §0.3.3 — Primary-Replica Principle (the architectural model this ADR honors)
- `docs/spec/sync.md` §22B.3.5 — legacy bridge conflict-resolution spec (carries the inherited gateway-wins rule implicitly; on the 2026-05-07 audit's RETIRED list; flagged here for awareness, not amended)
- Playbook §2 — ADR immutability rule (the reason ADR-0015's body is not edited; this ADR supplements rather than overwrites)

## Follow-ups filed (not gated by this ADR)

- **Stia's code PR.** Flip the equal-timestamp tie-break branch in `conflict_resolution.rs` (currently ~68-72) from `AcceptRemote` on tie to the equivalent local-keep variant; flip the matching unit test; verify on real two-machine setup as part of #1299's close criteria.
- **"Inherited from [retired system]" provenance sweep.** This ADR's experience suggests that any spec text framed as "behavior identical to X" or "same as bridge" is a flag for an absent decision. Reed-led sweep across spec to identify other inherited-without-deliberation rules; not gated by this ADR's launch. The pattern aligns with ADR-0041's "implementation-transition-gap" follow-up; could be folded into that sweep rather than a separate one.
- **§22B.3.5 retirement amendment** (if §22B's RETIRED-status work touches §22B.3.5's body). If a retirement spec touch ever edits §22B.3.5 prose, the implicit "newer wins / tie-to-gateway" framing in the bridge spec should reflect Pane-wins. Not required for launch (§22B is no longer active spec); flagged so the retirement work doesn't accidentally re-canonicalize the bug.
