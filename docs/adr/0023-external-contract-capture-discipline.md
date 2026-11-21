# ADR-0023: External-System Contracts Must Be Captured at Spec Time

**Status:** Accepted
**Date:** 2026-05-02
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The §22C Pane Gateway spec, drafted 2026-04-27, documented the gateway's Pane-facing HTTP/2 + SSE surface in detail and was silent on OpenClaw's WebSocket-RPC response shapes. That silence was a deliberate choice on incomplete reasoning: "the gateway absorbs OC's protocol asymmetry behind a clean Pane-facing surface" (the load-bearing argument in ADR-0015) was extended to "therefore OC's shapes are implementation details the gateway adapts and don't need to be in spec." The extension was not flagged as a separate decision.

The spec session also invented field names for the Pane-facing surface — `oc_agent_id`, `session_id`, `message_id`, `has_more`, `next_cursor` — based on spec-internal consistency rather than anything OC actually emits. Stia implemented the gateway against the spec; serde rename annotations bridged the spec's invented names to whatever Stia inferred OC would send; Reed reviewed the implementation against the spec.

Three independent conventions resulted: spec names, code names (via serde rename), and OC's actual names. None matched. The gateway's deserializers ran against OC and either hard-failed (when OC sent fields with names that didn't match the expected struct) or silently dropped data (when serde happily filled `Option` fields with `None` for fields OC sent under different names).

The §826 audit, run 2026-05-01, found 8 of 11 whitelisted OC RPC methods had field-name or shape mismatches between the gateway's expected struct and OC's actual response. 3 hard-failed at runtime; 5 silently drifted. The `agents.list` mismatch was the visible symptom that triggered the audit; the audit revealed the systematic gap.

The mechanism is general. Any spec that invokes an external system — third-party API, sibling service, OS interface, MCP server, OpenClaw RPC, BYOK provider — is exposed to the same failure mode if the external system's response contracts are inferred at spec time rather than captured from source-of-truth. Specifically:

- **Tier 2 BYOK providers** (OpenRouter, Anthropic, Ollama). Pane talks directly to provider APIs. The current `OpenRouterProvider` adapter presumes an OpenAI-compatible response shape; provider drift is unverified. Anthropic has changed `/v1/messages` response shapes multiple times during Pane's development.
- **Future MCP server integrations.** Each MCP server has its own response shapes. The MCP App pattern is "external system, opaque contract" by definition.
- **Future Tier 1 hosted infrastructure.** Auth provider, billing provider, OC fleet manager — same shape: external system, response contracts that need to be captured.

Without a checkpoint that requires external-contract capture at spec time, the same shape of bug ships on every external integration.

### What if we didn't decide

Three predictable failure modes:

- **The §22C/§826 pattern recurs at the next external integration.** Without an explicit spec-time checkpoint, the next spec session that invokes an external system makes the same authoring error. The §826 audit fixes the OC-specific instance; it does not prevent recurrence in other integrations.
- **The launch-time Tier 2 risk goes unaddressed.** Pane ships at launch with three BYOK providers. If their response shapes aren't captured anywhere and any provider has drifted, the same bug ships live on launch day with paying users present.
- **Reed's PR review continues operating without the canonical reference.** Reed can verify "PR matches spec," but if the spec doesn't include external-contract captures, "PR correctly adapts the external system" is unreviewable. The same gap that authored §22C's drift authors the next one.

## Decision

**External-system contracts must be captured at spec time.** Specifically:

1. **Capture is mandatory.** Every spec session that invokes an external system produces, as part of its output, the actual response shapes for every call the spec invokes. Captures come from the external system's source-of-truth: source code, official docs, or a live probe (curl, WS frame log, OS API documentation). Inferred shapes and plausible-looking field names are not captures.

2. **Captures live in spec, not in code comments.** The captured shape lives in the relevant spec subsection alongside the call that uses it. Code adapts the shape and references the spec capture; the spec capture references the source-of-truth. Single canonical reference per integration.

3. **Captures are versioned.** Each capture includes the source path, the external-system version it was captured from, the date, and a versioning note (stable / breaking-on-major / unstable). This bounds drift: a future audit compares the capture's version against the current external-system version and re-captures if changed.

4. **Capture format.** Inline JSON example in the relevant spec subsection. Format detail is in the playbook §1 patch landing alongside this ADR; this ADR pins the requirement, the playbook pins the format.

5. **Spec-time, not implementation-time.** The check happens at spec session output, before any ADR is filed against the spec or any implementation issue is queued. Spec sessions explicitly ask, for every external system the spec invokes, "do we have the actual response shapes? captured where? versioned how?" If the answer is "no" or "deferred to implementation," the spec session output is incomplete and the session does not lock.

6. **Spike-and-exit when capture isn't possible.** If shapes can't be captured for legitimate reasons (system not yet built, access restricted, vendor docs missing), the spec session files a spike issue and exits without lock. Specs that hand-wave external contracts are not ready.

7. **Reed PR-review checks the capture-to-code link.** For every external call a PR introduces or modifies, Reed verifies the spec cites a captured shape and the code matches that shape. Captured-but-stale shapes (external system version has moved past the captured version) are flagged for re-capture, not silently accepted. The Reed checklist line lands in the playbook §2 patch alongside this ADR.

## Consequences

### Positive

- **Closes the §22C-shape recurrence path.** Future external integrations get authored against a captured contract instead of an inferred one. The Stia + Reed degrees of freedom that produced §22C's three-convention drift go away.
- **Reed's PR review becomes meaningful for external calls.** The capture-to-code check gives Reed something concrete to verify. Currently Reed verifies "PR matches spec"; for external calls, "spec matches reality" was unverifiable.
- **Tier 2 launch risk becomes a tractable audit.** "Audit BYOK provider shapes" is bounded work — capture the current shape from each provider, compare to code, document version. The audit-scoping doc (`docs/audits/2026-05-02-tier-2-byok-shape-audit-scope.md`) makes the scope concrete.
- **Versioning policy bounds drift.** A capture marked "current as of OC v0.71.0" tells the next reader exactly when to re-verify. Without versioning, captures rot silently.
- **Pattern is general.** The discipline applies to every external integration the codebase has or will have. Not OC-specific, not gateway-specific.

### Negative

- **Spec sessions get longer.** External-contract capture is real work — read OC's source, run a curl, document the shape. For specs invoking many external calls, this can add hours to a session. Mitigated by: most specs invoke one or two external systems; the §22C case (11 OC methods) is unusually broad; the alternative (PR #826-shape recurrence) is more expensive.
- **Captures rot.** External systems version up; captures become stale unless re-verified. Mitigation: versioning policy on each capture (stable / breaking-on-major / unstable) tells readers when re-verification is due. Stable contracts age well; unstable contracts get re-captured each release.
- **"Source of truth" is sometimes ambiguous.** Source code is most reliable but not always accessible; official docs lag source; live probes capture one moment in time. Mitigation: capture cites whichever source-of-truth is most reliable at capture time, and the versioning note encodes the trust level (a capture from official docs is less reliable than one from source).
- **Discipline-only enforcement at spec time.** This ADR mechanizes the Reed checklist line at PR time but not the spec-session checkpoint itself. A spec session can still skip the capture step. Mitigation: Reed's PR review fails any PR whose external calls don't trace to a captured shape, which feeds back to the spec session — the spec gets sent back if its external contracts weren't captured. Mechanizing the spec-session checkpoint itself (a CI fixture that deserializes captured shapes into expected structs) is named as future work; not built in this ADR.

### Risks

- **Captures get treated as decoration, not contracts.** If captures are pasted into spec but never re-verified, they become decorative noise. Mitigation: Reed's PR review checks the capture's version against the external-system's current version; stale captures are flagged.
- **External systems with no source-of-truth access.** Closed-source vendors with poor docs (some smaller LLM providers, some MCP servers) can't be captured cleanly. Mitigation: live-probe captures are valid; "captured via curl on `<date>` from `<endpoint>`" is acceptable when source isn't accessible. Versioning note flags the lower trust level.
- **Spec sessions overcorrect and capture every internal contract too.** Internal sibling services (Pane↔gateway) don't need capture-from-source-of-truth because both sides are authored together. Mitigation: this ADR's scope is *external* systems — code authored outside the Pane repo. Internal contracts are governed by ordinary spec-vs-code review.
- **The Tier 2 audit reveals a launch-blocking drift.** If the audit (scoped in `docs/audits/2026-05-02-tier-2-byok-shape-audit-scope.md`) finds significant provider drift, fixing it before launch costs time. This ADR makes the risk visible, not absent. Better to surface and address now than ship and discover post-launch.

## Alternatives Considered

**Document OC's response shapes in §22C only; treat this as an OC-specific fix.** The minimal version: the §827 amendment captures OC's shapes, no general-purpose discipline added. **Rejected** because the mechanism that produced the §22C bug is general — every external integration is exposed. Fixing only the OC-specific instance leaves Tier 2 BYOK providers, future MCP servers, and Tier 1 infrastructure exposed to the same failure mode.

**Mechanize via CI fixture-based deserialization tests.** Same shape as ADR-0018's smoke check — for each captured shape, ship a fixture file; CI deserializes the fixture into the gateway's expected struct and fails if fields don't match. **Considered, deferred to future work.** The mechanization is sound (same pattern as ADR-0018 and ADR-0016) but premature: the audit scope (§826 + Tier 2) is large enough that capturing the shapes first, before mechanizing, lets us learn whether the discipline-plus-Reed-checklist layer is sufficient. If the discipline floor leaks after the captures land, mechanize then. Named as future work in this ADR.

**Defer the discipline change; address only the §826 audit findings.** Wait until the next external-integration incident before generalizing. **Rejected** because the next incident is the Tier 2 launch — we can predict it. Discipline-floor mechanization works because it gets ahead of the recurrence; deferring until recurrence is the failure mode ADR-0008 was filed to end.

**Make captures a code-level concern (typed bindings, generated structs from OpenAPI specs, etc.).** Skip the spec layer; require external integrations to import a generated client from the external system's spec. **Rejected** for two reasons: most external systems Pane integrates with don't publish machine-readable specs (OpenClaw, BYOK providers' streaming endpoints, OS APIs); and even when they do, the spec layer of capture-with-versioning-note is doing different work — it's making the contract visible to spec readers, not just to compilers. Code-level binding is complementary, not a substitute, and is itself a future-work item once captures exist.

**Scope the rule to only "high-risk" external systems.** Define "high-risk" (e.g., systems with mutable contracts, systems Pane is writing to) and apply the rule there only. **Rejected** because the §22C case shows that "low-risk read-only RPC" was wrong about its own risk — OC is a sibling project, presumed coordinated, and 8/11 calls drifted anyway. The risk classification step itself is the failure mode the rule prevents. Apply uniformly.

## References

- Spec section: `docs/dev-hygiene-playbook.md` §1 (Spec Discipline) — patched to add the external-contract checkpoint subsection alongside this ADR
- Spec section: `docs/dev-hygiene-playbook.md` §2 (Reed PR review) — patched to add the capture-to-code line
- Reference incident: §826 audit (filed by Brit, 2026-05-01) finding 8 of 11 OC methods drifted; specific surface bug was `agents.list` mismatch caught during gateway troubleshooting
- Origin spec session: §22C Pane Gateway (drafted 2026-04-27); the §827 amendment captures OC-specific shapes per this ADR's discipline
- Audit scoping: `docs/audits/2026-05-02-tier-2-byok-shape-audit-scope.md` — applies this ADR's discipline to Tier 2 BYOK providers before launch
- Related ADRs: ADR-0008 (mechanical dev hygiene — pattern this ADR follows); ADR-0015 (Gateway↔OC Transport — the ADR whose "absorb OC's protocol asymmetry" framing was over-extended into "OC's shapes need not be in spec"); ADR-0017 / ADR-0018 (PR #690 incident response — same shape: discipline failed, mechanization layered on; ADR-0018 specifically is the closest precedent for the future-work CI fixture mechanization)
- Future work: CI fixture-based deserialization test for captured shapes (mechanizes the Reed checklist line in CI), filed as a separate issue once captures exist
