# ADR-0038: Extraction Model Routing — Dedicated Setting and Fallback Chain

**Status:** Accepted
**Date:** 2026-05-18
**Amended:** 2026-05-19 — Decision 2's fallback chain replaced with per-feature lookup (no cross-feature fallback, no fallback to primary). Decision 3's fall-through-to-primary warning replaced with silently-disabled gate. Decision 5's "Use Smart Routing Model (recommended)" default dropped. See Amendment 2026-05-19 at bottom of file.
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (supersedes the 2026-02-28 Brit/Matt pinned decision in memory-pipeline.md §5.4, which was a `📌 Implementation Note` rather than an ADR; that note is replaced in the spec amendment that lands with this ADR)
**Supplements:** ADR-0010 (Three-Writer Memory Pipeline — extraction is the upstream input to ADR-0010's router), ADR-0037 (Settings UI Architecture — extraction picker follows ADR-0037's pattern)
**Related:** memory-pipeline.md §5.4 (Auto-Extraction), smart-routing.md §27.1 (Smart Routing Scope)

## Context

The memory extraction worker (`src-tauri/src/ai/extraction_worker.rs`) routes its LLM call to the primary AI provider — the same model the customer has configured for chat. This is implementation drift from spec intent in two places:

- **memory-pipeline.md §5.4 step 2** says "Send the conversation exchange ... to a Sonnet-tier model." The pinned `📌 Implementation Note (Brit/Matt decision, 2026-02-28)` further locked "Use Sonnet-tier models for extraction (not Haiku). Sonnet is reliable at structured extraction and cheap enough to run on every message (~$0.001 per extraction)." The spec describes a sized constraint (reliable at structured extraction, cheap per call) expressed via a model-family name; the worker honors neither the size constraint nor the model-family name — it uses whatever is configured as primary.
- **smart-routing.md §27.1 Scope** says "Background tasks (rerank, extraction, synthesis) use their own dedicated model settings and are NOT affected" by smart routing. The dedicated setting for extraction was named (`extraction_model`, as a future key in `ai_settings`, per the archived smart-routing spec §29.3) but never created. The `AiStateWrapper` is `Arc<Mutex<AiConfig>>`; the extraction worker reads `state.model.clone()` and dispatches via `call_llm_from_config` to whatever provider/model the customer set as primary.

Hardware testing on 2026-05-18 with primary configured as OpenClaw + Opus 4.6 surfaced the cost of the drift (issue #1182). Extraction calls hit a 30-second timeout, dropped the extracted facts, and produced empty daily-log entries downstream. The user-visible symptom was missing daily-log entries; the structural cause was extraction running through a model and provider path optimized for chat throughput, not structured JSON output under time pressure.

The corrective is to land the missing setting and the resolution path that connects it to the existing fallback infrastructure already used for `simple_tier_model`. The change is small in scope and large in user-visible reliability impact.

### What if we didn't decide

- **Extraction continues to time out on Opus-via-OpenClaw and any future high-latency primary configuration.** Daily-log entries continue to drop. The "memory is Pane's core differentiator" framing from the 2026-04-23 architecture decisions doc is contradicted by silent extraction failure at the primary user-visible memory surface.
- **The spec language stays self-contradicting.** memory-pipeline.md §5.4 says Sonnet-tier; smart-routing.md §27.1 says "their own dedicated model settings"; the archived smart-routing spec §29.3 names `extraction_model` as deferred. Three sections, three readings, none matching code.
- **ADR-0037's first concrete application doesn't ship.** ADR-0037 codifies the picker pattern; without a picker for extraction landing alongside, the architectural rule is abstract.

## Decision

### 1. New setting: `extraction_model`

A new key is added to the `ai_settings` table (key-value, TEXT/TEXT): `extraction_model`. The value is a provider-native model identifier — the same shape as `simple_tier_model`, `memory_search_llm_rerank_model`, and the primary `model`. No new column, no new table, no migration logic; `ai_settings` accepts new keys without schema change.

### 2. Fallback chain

The extraction worker resolves the model to use via the following chain. First match wins:

1. **`extraction_model`** — the new dedicated setting (this ADR).
2. **`simple_tier_model`** — the Smart Routing Model setting, already configured for fast/cheap background work in customers who have set up smart routing.
3. **`model`** — the primary AI model setting, the current de-facto extraction model. Used as the final fall-through when neither dedicated setting is configured.

The fall-through to the primary model preserves current behavior for customers who have neither setting configured — extraction continues to run; it just runs through whatever the primary provides. The advantage of the chain is that *any customer with smart routing configured automatically gets working extraction on this change*, with no new value required.

### 3. Failure mode on fall-through (step 3)

When extraction resolves to the primary `model` via step 3 (neither `extraction_model` nor `simple_tier_model` is set), the worker logs a warning at extraction time:

> `[EXTRACTION] No dedicated extraction model configured; falling through to primary AI model. This may cause timeouts on slow primary providers. Configure Extraction Model in Settings → AI for a faster default.`

The warning fires once per session at first fall-through (deduplicated, not per-extraction-call). The extraction call proceeds normally — the worker does not block on the missing setting.

### 4. Failure mode on configured-model failure (steps 1 or 2)

When extraction resolves to a configured model (steps 1 or 2) and the call fails (timeout, auth error, unknown model, provider 5xx, etc.), the worker does *not* retry through a different model in the chain. The failure is surfaced via the existing extraction error logging (§5.4 step 5 / `extraction_queue` retry semantics). The customer's configured choice is honored — silently falling back to a different model on configured-model failure would mask configuration errors the customer needs to correct.

Retry-with-different-model is reserved for the no-setting case (step 3 above), not the explicit-and-failing case.

### 5. Picker

A new picker in Settings → AI → Smart Features writes `extraction_model`. The picker reuses the existing `SearchableModelSelect` component. Default display when `extraction_model` is unset: **"Use Smart Routing Model (recommended)"** — making the step 2 fallback explicit and discoverable so most customers don't need to interact with the new picker.

Per ADR-0037 §3, this picker is the sole writer for `extraction_model`. No cross-reference from a feature tab is required in v1 because extraction is not a Memories-tab-surfaced feature in the current spec (the `memory_extraction_enabled` toggle from §5.4B is a separate setting concerning whether extraction runs at all, not which model handles it). If a future surface needs to display the configured extraction model, it uses ADR-0037 §4's greyed-out + pointer pattern.

### 6. Spec language becomes model-agnostic

memory-pipeline.md §5.4 step 2 is amended to read "the configured extraction model (see §5.4C)" — no model-family naming. The 2026-02-28 pinned `📌 Implementation Note` is replaced with a supersession-style `📌 Why This Exists` block citing this ADR and preserving the structural rationale (reliability at structured JSON output, low cost ceiling per call) without binding the spec to model family names that age fast. A new §5.4C codifies the resolution chain and storage shape.

smart-routing.md §27.1 Scope line is amended to name the now-built `extraction_model` setting and to reference this ADR.

## Rationale

### Why fall through to primary instead of blocking

The status quo is that extraction routes through primary today. Blocking extraction when the new settings are unset would *worsen* user-visible behavior at the moment this change lands — every customer who hasn't yet seen Settings → AI to configure the new value would see extraction stop. Fall-through preserves current behavior on the unconfigured path, lets customers with `simple_tier_model` configured get the win automatically, and surfaces the warning that the configuration is suboptimal without taking extraction offline.

### Why no retry-with-fallback on configured-model failure

A customer who explicitly sets `extraction_model` to a specific provider/model wants extraction to use that model. Silently falling through to a different model on failure hides the configuration problem and produces extraction calls the customer didn't expect. Surface the failure; let the customer fix the configuration.

### Why this ADR pre-launch, not later

Zero customers today. The fallback chain protects the few existing test customers who haven't configured the new setting. Post-launch the same fallback would require thinking about migration (do we backfill `extraction_model` from a default? do we prompt customers?). Landing the chain before any customer has the unconfigured state baked in makes the post-launch story simply "configure it if you want; default behavior is preserved."

### Why this ships as ADR-0038, not as a §5.4 spec touch alone

The decision is two-part: the spec change is the visible artifact, but the architectural choice (dedicated setting + fallback chain + model-agnostic spec language) is what needs to survive into post-launch reasoning. The pinned decision being superseded — Brit/Matt's 2026-02-28 "use Sonnet-tier" — is an architectural choice that wants a successor decision documented, not just edited out of the spec. ADR-0038 is the durable successor.

## Consequences

### Positive

- **Extraction reliability fix lands without forcing every existing customer to reconfigure.** Customers with `simple_tier_model` configured get working extraction immediately; customers without it keep current behavior plus a warning.
- **Spec stops naming specific model families.** memory-pipeline.md and smart-routing.md become durable across model releases.
- **ADR-0037's pattern ships with a concrete first application.** The picker is the test of the architectural rule.
- **Failure surface is honest.** Configured-model failures surface as failures, not as silent fallback to a different model.

### Negative

- **One more setting in Settings → AI.** Customer-facing surface grows. Mitigated by clear naming and the "Use Smart Routing Model (recommended)" default display reducing the need for most customers to interact with the new picker.
- **Three-way fallback chain is more logic than a single setting read.** Cost is small (three `get_ai_setting` calls in worst case, all keyed lookups in a small in-memory table) but is more surface area than a single setting.
- **The deduplicated warning is a small stateful primitive in the extraction worker.** "Once per session" requires a flag. Acceptable cost; the alternative (warning on every extraction call) is log noise.

### Risks

- **A customer sets `extraction_model` to a model their internal provider doesn't expose.** Mitigated because the picker reads its options from the internal provider's model list (same as `simple_tier_model`); custom-entry option exists but is the customer's explicit choice. Extraction worker logs the resolved-model and the failure when the call fails; customer sees the configured model name in the failure context.
- **The fallback chain produces unexpected behavior on multi-provider configurations.** Imagined: a customer with primary on OpenClaw + internal provider on OpenRouter sets `extraction_model` to an OpenRouter model. Today's extraction worker dispatches via `call_llm_from_config` which respects the primary provider; the right fix is to dispatch extraction through the internal provider when configured, consistent with the archived spec §29.4–29.5 provider-resolution chain used for other background tasks. **Filed as a follow-up not gated by this ADR:** extraction provider dispatch parity with rerank. The decision in this ADR is about *which model identifier* to use; *which provider* dispatches it is a related-but-separable concern.
- **Synthesis ships and reuses the same setting accidentally.** Mitigated by ADR-0037 §3's one-key-per-role rule plus explicit naming of `synthesis_model` as a separate future key in the smart-routing.md §27.1 amendment.

## Alternatives Considered

**Hard-code a "Sonnet-tier" model family in Pane code.** Closest to the 2026-02-28 decision read literally. **Rejected** because (a) Sonnet is a model name tied to one provider (Anthropic); customers on OpenRouter, Ollama, OpenClaw, or future providers can't map "Sonnet-tier" without Pane shipping a model-family heuristic; (b) model families and names change on a multi-month cadence; the spec ages with them; (c) Pane's design principle is that the customer picks models from their configured provider — encoding family knowledge in Pane breaks the principle. The "what makes a good extraction model" reasoning lives in this ADR's Rationale; the runtime choice is the customer's.

**Single setting, no fallback chain — `extraction_model` required.** **Rejected** because it would force every customer to configure the new setting before extraction works. Worse status quo than today for any customer who hasn't seen the new picker.

**Add `extraction_provider` alongside `extraction_model`.** Lets the customer override the *provider* extraction uses, not just the model. **Rejected for v1** because it expands scope significantly and is solving a different problem (provider dispatch parity with rerank). Filed as a follow-up under "Risks." Keeping this ADR scoped to the model-identifier question keeps the fix small.

**Block extraction until the new setting is configured.** Forces all customers to make an explicit choice. **Rejected** because (a) it converts a silent bug into a visible regression on update; (b) the fall-through preserves current behavior in the no-setting case, which is the right pre-launch default; (c) blocking behavior on configuration is a heavier-handed pattern than the project has used elsewhere.

**Auto-set `extraction_model` to `simple_tier_model`'s value at first launch after this change.** Simulates "explicit override" by populating the new key from an existing one. **Rejected** because it creates a divergence the customer didn't ask for — if they later change `simple_tier_model`, the auto-set `extraction_model` value doesn't follow. The fallback chain expresses the same intent (use simple-tier-model unless overridden) without copying state.

**Have extraction read the rerank model's key.** Reuse `memory_search_llm_rerank_model` for extraction too on the theory that both are "fast/cheap background tasks." **Rejected** because it violates ADR-0037 §3 (one canonical key per role) and conflates two roles a future customer may legitimately want to configure differently — rerank handles short scoring prompts, extraction handles longer structured-JSON prompts; the optimal model can diverge.

## References

- Issue #1182 — extraction silently routes to primary chat model; the pre-launch hardware test that surfaced the gap
- ADR-0037 — Settings UI Architecture (the picker for `extraction_model` follows ADR-0037's pattern)
- ADR-0010 — Three-Writer Memory Pipeline (extraction is the upstream input to ADR-0010's router; routing depends on extraction completing reliably)
- ADR-0008 — Mechanical Dev Hygiene Over Discipline-Based Rules (the round-trip-key-reader check that would have caught the dead-key precedent referenced in ADR-0037)
- `docs/spec/memory-pipeline.md` §5.4 — amended by this ADR (step 2 + pinned decision)
- `docs/spec/memory-pipeline.md` §5.4C — new subsection codifying the resolution chain, lands with this ADR
- `docs/spec/smart-routing.md` §27.1 — Scope line amended to reference this ADR
- 2026-02-28 Brit/Matt decision (memory-pipeline.md §5.4 pinned note) — superseded by this ADR's Decision 6; the structural rationale survives, the model-family naming is removed
- `docs/archive/smart-model-routing-spec.md` §29.3 — original naming of `extraction_model` as a future key; this ADR closes that future
- `docs/audits/485-extraction-pipeline-audit.md` — historical extraction failure mode (provider-format model name corruption); separate root cause from #1182 but in the same code path

## Follow-ups filed (not gated by this ADR)

- **Extraction provider dispatch parity.** Today extraction dispatches through `call_llm_from_config` which respects the primary provider. Rerank uses the internal provider when configured (archived spec §29.4–29.5). Extraction should follow the same provider-resolution chain so that `extraction_model` choices on the internal provider's model list dispatch through the internal provider, not the primary. File as a separate issue; not a Pane-side blocker for ADR-0038.
- **Synthesis model setting.** ADR-0010 §5.5.3 names a future synthesis model role. When synthesis is hardened for production use, `synthesis_model` lands under the same pattern (this ADR's fallback chain + ADR-0037's picker placement).

---

## Amendment 2026-05-19: Per-feature lookup, no cross-feature fallback, no primary fallback

### Background

The original ADR-0038 (2026-05-18) specified a model resolution chain inside Pane's `AiConfig`:

1. `extraction_model` → 2. `simple_tier_model` → 3. primary `model`

The chain shipped in v0.65.37 (#1185) and was marked hardware-verified on the same day. The verification was reading Pane's intent logs (`source: extraction_model | simple_tier_model | primary`), not execution. Reed audit 2026-05-19 (`docs/audits/2026-05-19-pr1186-gateway-model-handling-audit.md`, Addendum 2) traced the path empirically: Pane resolves the model name correctly, sends it to the primary provider, and — for OpenClaw users — the gateway at `chat.rs:37` unconditionally rewrites the model field to `"openclaw"` before forwarding. The intent log shows the resolved model; the gateway dispatches whatever OpenClaw's default agent is configured for (Opus 4.6 at time of audit). For ~$0.001/Haiku-extraction the customer is silently billed ~$0.18/Opus-extraction per qualifying message.

The Phase 1 product decision (2026-05-19) reframes the architecture:

- **Extraction is core.** Powers memory writes and daily-log entries. Cannot be silently degraded.
- **Extraction requires a direct internal provider.** OR, Anthropic, or Ollama. Routes via §29 (smart-routing.md §27.6), bypassing the gateway entirely.
- **No fallback to primary.** The $0.18/call Opus extraction risk that fallback created is unacceptable at scale. Customers without an internal provider don't get extraction.
- **Each background AI feature gates on its own model setting independently.** No cross-feature fallback. `extraction_model` null means extraction disabled; it does not silently switch to `simple_tier_model`.
- **Smart Features setup becomes part of onboarding.** The cost framing requires explicit opt-in at first run rather than implicit fallback. `docs/spec/onboarding.md` §6.4 (PR #1202) covers the flow.

### Supersedes

**Decision 2 (fallback chain):** Superseded. Extraction reads `extraction_model` only. If `extraction_model` is null, extraction is silently disabled — no fallback to `simple_tier_model`, no fallback to primary `model`. The same per-feature rule applies to Smart Routing (`simple_tier_model`) and Rerank (`memory_search_llm_rerank_model`); each feature reads its own canonical key and gates on that key alone.

**Decision 3 (fall-through warning):** Superseded. There is no fall-through. When `extraction_model` is null, the worker silently skips extraction — no toast, no banner, no log warning beyond a single info-level startup line noting that extraction is disabled. The original "step 3 warning" copy is retired.

**Decision 5 (picker default):** Revised. The Extraction Model picker in Settings → AI → Smart Features no longer offers "Use Smart Routing Model (recommended)" as a default option. The picker is model-list-only — customer picks a specific model from the internal provider's model list, or the picker is in its disabled/empty state. Null = disabled, period. Settings cleanup is filed for #1199 (see companion Settings cleanup spec).

### Unchanged

**Decision 1 (the `extraction_model` setting itself):** Unchanged. The key exists, the picker writes it, the worker reads it.

**Decision 4 (failure mode on configured-model failure):** Unchanged in shape. Configured-model failures still surface via existing extraction error logging; no retry-through-a-different-model. The amendment narrows the scope: there's only one branch (steps 1) instead of two (steps 1 or 2), because steps 2 and 3 no longer exist.

**Decision 6 (model-agnostic spec language):** Unchanged.

### Dispatch path (forward reference)

Extraction dispatches via the §29 internal-provider path codified in smart-routing.md §27.6 (revised 2026-05-19 to normative form). When `extraction_model` is set, the worker resolves the internal provider via §27.6's resolution chain (OR / Anthropic / Ollama) and calls `call_llm` direct — bypassing the gateway entirely for OpenClaw users. The model field is honored end-to-end; no overwrite at `chat.rs:37` because the gateway is never in the path.

### Process pattern

This amendment exists because the original ADR-0038 was scoped to "which model identifier does extraction use" and treated "which provider dispatches it" as a separate-and-deferred concern (filed under Risks → "Extraction provider dispatch parity"). The deferred concern turned out to be the load-bearing one: getting the model identifier right inside Pane has no effect if the gateway rewrites it. Same shape as #1167/#1176/#1177 — a fix that didn't carry across an architectural transition (in this case, the gateway boundary). Filed as a post-launch playbook addition: **spec work touching AI calls must explicitly trace which side of the §29 dual-provider boundary the call lives on, with execution-log evidence not intent-log evidence**.

### References (amendment)

- Reed audit 2026-05-19: `docs/audits/2026-05-19-pr1186-gateway-model-handling-audit.md` (Addendum 2 = empirical confirmation that gateway overwrites the model field for OC users)
- Phase 1 product decisions (2026-05-19): no primary fallback, three internal providers (OR / Anthropic / Ollama), Smart Features opt-in onboarding
- `docs/spec/smart-routing.md` §27.6 — normative dual-provider routing, revised 2026-05-19 from stub
- `docs/spec/memory-pipeline.md` §5.4C — extraction model resolution, revised 2026-05-19 to per-feature lookup
- `docs/spec/onboarding.md` §6.4 — Smart Features onboarding step (PR #1202)
- ADR-0037 §3 — amended 2026-05-19 to document onboarding-picker exception
- Issue #1201 — extraction routing implementation (consumes this amendment)
- Issue #1186 — closed via #1201 when extraction is no longer gateway-proxied
