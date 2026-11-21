# ADR-0037: Settings UI Architecture — Model Selection vs Feature Behavior

**Status:** Accepted
**Date:** 2026-05-18
**Amended:** 2026-05-19 — Decision 3 sub-clause added documenting the Smart Features onboarding step as a deliberate exception. Onboarding pickers write canonical keys (no §3 violation) and are justified by opt-in setup + per-feature gates.
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0010 (Three-Writer Memory Pipeline — §5.5.3 names a future synthesis model role that follows this ADR), ADR-0008 (Mechanical Dev Hygiene Over Discipline-Based Rules — round-trip-key-check is a candidate future mechanization)
**Related:** ADR-0038 (Extraction Model Routing — first concrete application of this ADR)

## Context

Pane has accumulated multiple AI-adjacent settings surfaced in two Settings tabs: an AI tab (sections "AI Connection" and "Smart Features") and a Memories tab. Without a pinned architectural rule for which settings live where, the pattern has drifted, producing three concrete failure modes.

**Drift mode 1: decorative pickers writing dead keys.** The AI tab's Smart Features section includes a "Rerank Model" picker (added in the v0.33.0 redesign, 2026-04-14) that writes `rerank_model` to `ai_settings`. The rerank backend reads `memory_search_llm_rerank_model`, not `rerank_model`. The AI tab Rerank Model picker has been decorative — writing a key no code path reads — since v0.33.0 shipped. Memory reranking has been silently configured only by the Memories tab picker (which writes the canonical key), regardless of what the customer picked in the AI tab. Source-read on 2026-05-18 confirmed: `simple_tier_model` (30+ backend refs, live), `rerank_model` (zero backend refs, dead), `memory_search_llm_rerank_enabled` (live, read from both tab surfaces using the same key).

**Drift mode 2: two pickers for one role.** Beyond the dead-key problem, two pickers for the same backend role (one in AI tab, one in Memories tab) create ambiguity for customers about where to set the value and for future development about where to add or modify. The v0.33.0 audit verified the AI tab picker existed, was searchable, used the right component, and saved on selection — but did not verify that the saved key was a key the backend read. The round-trip wasn't a load-bearing review check.

**Drift mode 3: enable toggle and model picker collapsed into one tab.** The v0.33.0 AI tab Smart Features section also includes an "Enable Memory Reranking" toggle, which writes the canonical `memory_search_llm_rerank_enabled` key — the same key the Memories tab's toggle writes. Two live writers for the same key on two surfaces. Unlike drift mode 1, the AI tab enable toggle *is* functional (writes the canonical key the backend reads). But it confuses the architectural split between "what model handles this feature" (a model-selection concern) and "whether this feature runs at all" (a feature-behavior concern). The two concerns are operationally distinct — customers may want to configure a rerank model in advance of enabling it, or disable rerank temporarily while keeping the model choice — and locating both controls on the same surface elides the distinction.

A fourth gap is structural: the extraction worker has no dedicated model setting at all (ADR-0038). memory-pipeline.md §5.4's pinned decision (2026-02-28) referenced "Sonnet-tier" by name, and smart-routing.md §27.1 referenced extraction as using "its own dedicated model setting" — but the setting was never created. The architectural question therefore applies to a setting that doesn't yet exist (where should the picker live?) and to a future synthesis model setting (ADR-0010 §5.5.3) that will eventually need the same answer.

### What if we didn't decide

- **Drift mode 1 recurs on the next picker added.** ADR-0038 introduces an `extraction_model` picker. Without an architectural rule, the picker could land in the Memories tab (because extraction is a memory feature) or the AI tab (because it's a model), and the next picker after that re-asks the same question. The pattern is local-correct, system-incoherent.
- **Drift mode 3 recurs on each new feature with both a model picker and an enable toggle.** Synthesis will eventually have both. Without an explicit rule, the toggle could land alongside the picker in the AI tab (mirroring rerank's v0.33.0 mistake) and customers would have two surfaces writing the same enable key — same shape as the dead-key drift mode 1, just with both writers live instead of one decorative.
- **The future synthesis picker has no decided home.** ADR-0010 §5.5.3 names a future synthesis model role. Without this ADR, that role's UI home is open every time the conversation is reopened.
- **Round-trip verification stays implicit.** The drift mode 1 failure was invisible to PR review at v0.33.0 because no convention required verifying that a written key has a backend reader. Future pickers can drift the same way unless the rule is explicit.

## Decision

### 1. Model pickers live in the AI tab.

Every setting whose value is *a model identifier consumed by an LLM call* lives in the AI tab. Today this includes:

- Primary AI model (`model`)
- Smart Routing Model (`simple_tier_model`)
- Rerank Model (`memory_search_llm_rerank_model`)
- Extraction Model (`extraction_model`) — new, per ADR-0038
- Future synthesis model (`synthesis_model`) — ships under this rule when ADR-0010 §5.5.3 lands

The AI tab is the single home for "which model handles X." Customers learn one location.

### 2. Feature behavior lives in the feature tab.

Toggles, sliders, thresholds, and other settings whose value is *not* a model identifier live in the tab that surfaces the feature. Examples:

- Memories tab: `memory_search_llm_rerank_enabled`, `memory_search_keyword_weight`, `memory_search_temporal_half_life_days`, `memory_search_fuse_both_sources`, and other search-tuning controls (memory-pipeline.md §5.15.6)
- Memory Settings (§5.4B): memory toast toggle, memory extraction on/off
- (Future) Notifications tab: cadence and channel toggles per §19

Behavior governs how the system *uses* the configured model; model selection governs *which* model is used. The split keeps both kinds of settings near the customer's mental model for changing them.

### 2a. Enable toggles for features live on the feature tab only.

A feature whose model picker lives in the AI tab does not also expose an enable toggle in the AI tab. The picker is configuration of *what model* the feature uses; the toggle is configuration of *whether the feature runs at all*. These are distinct concerns and live in different surfaces. A model picker remains functional even when the corresponding feature toggle is off — customers can configure ahead of enabling, or disable temporarily without losing the model choice.

The Memories tab is memory reranking's canonical enable-toggle home (`memory_search_llm_rerank_enabled`). The AI tab "Enable Memory Reranking" toggle introduced in v0.33.0 is an architecture violation under this rule and is removed in implementation. The Memories tab toggle is unchanged — it has always been the canonical writer of the enable key, and the source-read on 2026-05-18 confirmed the key flows live from both surfaces today.

Where a feature has no dedicated feature tab (e.g., smart routing, whose feature surface is chat itself and which has no separate Settings home), the AI tab serves as both the model-selection home and the feature-behavior home for that feature. The asymmetry maps to whether the feature has a separate tab; for smart routing, the AI tab *is* the feature tab.

### 3. One canonical `ai_settings` key per backend role. Decorative pickers are not permitted.

For each backend behavior governed by a model setting, exactly one `ai_settings` key is the source of truth. The picker that writes it lives in the AI tab. No other picker, anywhere in the UI, may write the same role under a different key.

The dead `rerank_model` key written by the v0.33.0 AI tab Rerank Model picker is retired by this ADR. The canonical rerank key is `memory_search_llm_rerank_model` (the key the backend has always read). The AI tab Rerank Model picker is retargeted to write the canonical key. No data migration is performed — at pre-launch volume customers re-pick the model once if needed.

The single-source-of-truth-per-role rule is enforceable on PR review: every new picker PR must cite the backend call site that reads the key it writes. PR review verifies the round-trip. A picker that cannot name a reader is a decorative picker — the v0.33.0 Rerank Model picker is the canonical example of this anti-pattern — and is rejected at review. Mechanizing the check (CI grep pairing `set_ai_setting('<k>', ...)` with `get_ai_setting('<k>')` reachability) is a candidate future hardening per ADR-0008's discipline-vs-mechanization precedent; filed as post-launch.

### 3a. Onboarding pickers — documented exception (amended 2026-05-19).

§1 establishes Settings → AI as the canonical home for model pickers. The Smart Features onboarding step at `docs/spec/onboarding.md` §6.4 hosts three model pickers — Extraction Model, Smart Routing Model, Rerank Model — at first-run setup. These onboarding pickers write the same canonical `ai_settings` keys (`extraction_model`, `simple_tier_model`, `memory_search_llm_rerank_model`) that the Settings → AI pickers write. They are not decorative under §3 and they do not introduce alternate keys.

The onboarding-as-second-surface exception is justified by two product constraints that don't apply to Settings:

1. **Opt-in setup.** Smart Features is opt-in at onboarding (per Phase 1 product decision, 2026-05-19). Each background AI feature gates on its own model setting being non-null (per-feature lookup rule, ADR-0038 amendment 2026-05-19). Null = feature silently disabled. The customer must therefore make explicit picks at first run for any features they want active — implicit defaults are not available.
2. **Per-feature gates can't be implicit.** A blank picker in Settings is a recoverable state — the customer goes to Settings → AI and fills it in when they realize a feature is off. A blank picker on first run is a *learned* state — the customer who doesn't set extraction at onboarding doesn't know later that memory learning is silently disabled. Onboarding forces the picks because the feature gates depend on them.

Settings → AI remains the canonical home for ongoing changes. Customers who change providers, swap models, or revisit the configuration do so in Settings, not by re-running onboarding. Onboarding is the first-write surface; Settings is the ongoing-write surface; both write the same canonical key.

The exception is bounded:

- **Same keys, no aliases.** Onboarding writes `extraction_model`, `simple_tier_model`, `memory_search_llm_rerank_model`. No new keys, no onboarding-specific suffix, no transitional state.
- **Round-trip rule still applies.** PR review for the onboarding pickers must cite the backend call site that reads the key, same as Settings pickers (§3 primary rule).
- **No third surface.** Pickers do not appear in any other surface — not in Memories tab, not in chat UI, not in command palette. The exception is specifically for onboarding and Settings; new surfaces require a new ADR amendment.

Future opt-in feature clusters with per-feature-gate semantics may invoke this same exception by citing it. The rationale (opt-in + per-feature gate forces explicit picks at first run) is the load-bearing reason — not the convenience of having a setup wizard.

### 4. Cross-references from feature tabs use the greyed-out + pointer pattern.

When a feature tab surfaces a behavior whose model selection lives in the AI tab, the feature tab shows the configured model name *greyed out, read-only*, with the inline pointer "Set in Settings → AI." The customer can see what model is configured without leaving the tab and has a one-tap path to change it.

The greyed display reads from the canonical key the AI tab picker writes — same source of truth, two surfaces.

Feature tabs never host model pickers, enable toggles for features whose picker lives in the AI tab, or any input that writes a model-role key. The cross-reference label is read-only display only — the only model-config surface a feature tab may present.

Current cross-references:

- **Memories tab → Rerank Model.** Greyed display of `memory_search_llm_rerank_model` (the value the AI tab Rerank Model picker writes per Decision 3). Pointer: "Set in Settings → AI." The rerank toggle (`memory_search_llm_rerank_enabled`, per Decision 2a) and rerank-tuning sliders remain in the Memories tab per Decision 2.

The Memories tab Rerank Model picker that has been the de-facto live picker since the v0.33.0 redesign is converted to this greyed display.

### 5. Forward-compat for synthesis and other future model roles.

The synthesis model setting named in ADR-0010 §5.5.3, and any model setting introduced post-launch for a new background AI task, follow the same pattern by default: picker in AI tab; if a feature tab surfaces the behavior, cross-reference via greyed-out display + pointer (Decision 4); if the feature has a dedicated tab and exposes an enable toggle, the toggle lives on the feature tab (Decision 2a), not the AI tab. No new architectural decision is required for each new role.

## Rationale

### Why model pickers in AI tab, not feature tabs

Model selection has a property feature behavior does not: model identifiers are shared infrastructure. The same model can be used for chat, smart routing, and any number of background tasks. Customers thinking "which model is configured" want one location; customers thinking "what should the rerank threshold be" want it next to the rerank toggle. The split tracks the customer's mental model rather than the feature-ownership graph.

### Why enable toggles live on feature tabs, not alongside their pickers

A picker configures *what*; a toggle configures *whether*. The two concerns are operationally separable: customers may want to choose an extraction model in advance of enabling extraction, or disable reranking for a session without losing the model choice. Co-locating both on the same surface elides the distinction and pushes customers toward thinking of model selection and feature enablement as the same operation. They aren't.

Beyond the conceptual split, co-locating both controls reproduces the dead-key shape on the enable axis. If both AI tab and feature tab host live writers for the same enable key (the v0.33.0 reality for rerank), a future redesign of one surface can drift the key without noticing — same risk profile as drift mode 1, different axis. Decision 2a forecloses that path: enable toggles live in exactly one place, the feature tab.

The smart-routing exception in Decision 2a (where the AI tab serves both roles) is bounded: smart routing has no separate feature tab and no other home for its enable toggle. The rule maps to "where does the feature live," not to "model and enable must be separated regardless."

### Why one canonical key, not key migration

At pre-launch volume (zero customers today), there is no migration concern. Future ADRs that introduce new roles also introduce new keys cleanly. The discipline this ADR enforces — one key per role, round-trip-verified at PR review — is more valuable than the cleverness of supporting alternate keys.

### Why call out the decorative-picker anti-pattern explicitly

The v0.33.0 audit verified the rerank picker existed, was searchable, used the right component, and saved on selection. The audit did not verify that the saved key was a key the backend read. That gap is what allowed the picker to ship and stay broken for a month-plus. Calling out "decorative picker writing dead keys" as an explicit anti-pattern makes the round-trip check load-bearing on future PR review, not optional. This is the discipline-floors-leak pattern from ADR-0008's family: the rule has to be explicit before it can be enforced, and enforced before it can be mechanized.

### Why the greyed-out pattern, not a duplicate picker

A duplicate live picker is what produced drift mode 1. A read-only greyed display is unambiguous: it shows what is configured, points to where to change it, and cannot be the source of a write that diverges from the canonical key. The pattern is lightweight to implement and consistent with other in-app "configured-elsewhere" surfaces.

## Consequences

### Positive

- **One predictable home for model selection.** Customers and developers know where to look.
- **Clear two-way separation between model-config and feature-behavior surfaces.** AI tab is model-config only (no enable toggles for features with dedicated tabs); feature tabs are feature-behavior only (no model pickers or model-role-key writers). Each surface has a single, well-bounded purpose.
- **Drift mode 1 cannot recur.** PR review requires every picker to cite its backend reader; decorative pickers are rejected.
- **Drift mode 3 cannot recur.** Decision 2a forecloses the AI-tab-hosts-enable-toggle pattern that the v0.33.0 redesign normalized.
- **Future model roles have a default UI home.** Synthesis (ADR-0010 §5.5.3) and any new background AI task ship without re-litigating the architecture question.
- **Cross-tab consistency.** Feature tabs surface the configured model without holding state that can diverge from the canonical key.

### Negative

- **The Memories tab Rerank Model picker is downgraded to a read-only display.** Customers who learned to set rerank model in Memories now click through to AI tab. v1 customers don't have this muscle memory; the change is invisible to them. Pre-launch acceptable trade.
- **The AI tab "Enable Memory Reranking" toggle is removed entirely.** Customers who learned to enable reranking from the AI tab now find the toggle in the Memories tab. v1 customers don't have this muscle memory either; the change is invisible to them.
- **Two additional rules for PR review.** Every new picker PR must name its backend reader (Decision 3); every new feature with both a model picker and an enable toggle must split them per Decision 2a. Adds two small audit steps. The cost of *not* enforcing them produced a month-plus of silent failure on rerank and a duplicated enable-toggle surface that would have invited a future dead-key drift on the enable axis.
- **Tab semantics now load-bearing.** "AI tab is for model selection; feature tabs are for behavior and feature-enable controls" becomes a structural assumption. Future Settings redesigns must preserve the split or supersede this ADR.

### Risks

- **A future role doesn't fit cleanly into "model selection vs feature behavior."** Imagined example: a per-project model override the customer expects to set per-project, not in the global AI tab. Mitigation: when a future role surfaces this tension, file a clarifying-supersession ADR per playbook §2 (ADR-0019 / ADR-0020 precedent) rather than splitting the rule silently. The bar for supersession is "the existing rule produced drift."
- **A cross-reference's greyed display drifts from the canonical key.** Mitigation: the greyed display reads from `ai_settings` at render time, not from a separate cache. PR review verifies the read path is the canonical key.
- **A future feature without a dedicated feature tab introduces an enable toggle in the AI tab and erodes Decision 2a's smart-routing exception.** Mitigation: the exception is named explicitly (smart routing has no other home). Future features without dedicated feature tabs should justify the lack of a feature tab — not auto-inherit the AI-tab-as-feature-home pattern.
- **PR review enforcement is human, not mechanical.** Mechanization is filed as a post-launch candidate per Decision 3. Until then, the discipline is a review-floor and subject to the same erosion pattern any review-floor faces.

## Alternatives Considered

**Model pickers live next to the feature that uses them.** Rerank picker in Memories, extraction picker in Memory Settings, etc. **Rejected** because it scatters model selection across the UI and reproduces drift mode 2 for every multi-consumer model (Smart Routing Model is used by simple-tier chat and as a default for several background tasks — which feature does it "belong to"?). Customers' mental model collapses across model-selection contexts.

**Model pickers everywhere, syncing to one key.** Both AI tab and Memories tab have live rerank pickers; both write to the same canonical key. **Rejected** because two write surfaces for one key create the v0.33.0-shape regression risk: a future redesign of one surface drifts the key without noticing, and the other stays correct silently. One write surface, multiple read surfaces, is the safer asymmetry.

**Enable toggles live with their pickers in the AI tab.** Inverse of Decision 2a. Memories tab loses the rerank enable toggle; AI tab keeps both picker and toggle for each feature. **Rejected** because (a) it reproduces drift mode 3's two-live-writers-for-one-key shape if any feature tab retains a duplicate toggle, (b) it confuses model-config with feature-behavior in customers' mental model, and (c) it forecloses the legitimate use case of configuring a model in advance of enabling the feature without binding the two actions in time.

**One canonical key, but no greyed-out display.** Feature tabs simply don't show the configured model; customer must navigate to AI tab to see what's configured. **Rejected** because the configured model is useful context for tuning behavior (a customer adjusting rerank threshold benefits from seeing which model the threshold applies to). The display cost is low; the navigation friction without it is real.

**Defer until the third picker.** Wait for synthesis to ship and decide all three at once. **Rejected** because ADR-0038's extraction picker ships imminently and would otherwise choose a home ad-hoc. Deciding now, with extraction as the first concrete application, is cheaper than landing extraction with a stub and revisiting.

**Mechanize the round-trip rule with a CI check first, then file the ADR.** **Rejected** because the rule is the load-bearing decision; mechanization is enforcement. The ADR documents the rule even if mechanization slips to post-launch. Filing the ADR first lets PR review enforce manually now and gives mechanization a target to validate against.

## References

- Issue #1182 — extraction silently routes to primary chat model; the source-read that surfaced drift modes 1 and 3 alongside the extraction-routing gap
- ADR-0038 — Extraction Model Routing (first concrete application of this ADR's UI architecture)
- ADR-0010 — Three-Writer Memory Pipeline (§5.5.3 names a future synthesis model role that follows this ADR's pattern)
- ADR-0008 — Mechanical Dev Hygiene Over Discipline-Based Rules (precedent for mechanizing PR-review rules; round-trip-key-check is filed as a post-launch mechanization candidate per Decision 3)
- `docs/audits/v0.33.0-review.md` — the audit that landed the v0.33.0 AI tab Rerank Model picker and Enable Memory Reranking toggle and verified UI properties but not key round-trip or surface-correctness for the toggle; cited as the failure mode this ADR addresses on both axes
- `docs/spec/memory-pipeline.md` §5.4C — extraction model resolution, lands per ADR-0038 with the picker per this ADR
- `docs/spec/memory-pipeline.md` §5.15.6 — rerank settings; amended by this ADR to align with Decisions 1, 2, 2a, 3, 4
- `docs/spec/smart-routing.md` §27.1 — scope line amended to name the now-built `extraction_model` setting
- `docs/spec/smart-routing.md` §27.3 — UI surface section amended by this ADR to codify the AI tab structure, cross-reference pattern, and never-host rule
- 2026-05-18 source-read (Brit): `simple_tier_model` 30+ backend refs (live); `rerank_model` zero backend refs (dead); `memory_search_llm_rerank_enabled` live with same key on both tab surfaces. Resolves audit Issue B from the 2026-05-18 spec session brief.

## Follow-ups filed (not gated by this ADR)

- Post-launch: CI mechanization of the round-trip-key-check (Decision 3, "Risks"). Pair `set_ai_setting('<k>', ...)` writers with `get_ai_setting('<k>')` reachable readers; fail PR if a written key has no reader. References ADR-0008.
- Post-launch: CI mechanization of the enable-toggle-surface check (Decision 2a). Verify that each `_enabled` key (or equivalent feature-toggle naming convention) has exactly one writer surface. Same shape as the round-trip check but on the enable axis.
