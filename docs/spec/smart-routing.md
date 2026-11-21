# Smart Model Routing

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §27

> **Cross-references:** See [ai-integration.md](ai-integration.md) §4 for AI Tool Layer details.

---

## 27. Smart Model Routing
> **Status:** BUILT (global-only). Per-project routing overrides are RETIRED — the per-project UI shipped in v0.25.1 (#384 Phase 2), was removed in v0.32.3 (`8a7f6a7c`), and the retirement is ratified by ADR-0061 (2026-08-16). Historical build issue: #384. Retirement spec pass: #2534.
> **Cross-references:** §4 (AI Tool Layer), §21 (Schema), ADR-0061
> **Historical origin:** `docs/archive/smart-model-routing-spec.md` — do not cite as live spec (§27.6.4)

Smart model routing addresses cost optimization for AI-powered chat. A single-model approach costs ~$6K/mo for what intelligent routing delivers at $40/mo by using cheaper models for simple tasks and expensive models only when needed.

### 27.1 Overview

**Three-tier routing:**
- **Tier 1 (Simple):** Haiku — short Q&A, factual lookup, formatting
- **Tier 2 (Standard):** Sonnet — multi-turn conversation, coding, creative writing
- **Tier 3 (Complex):** Opus — architecture design, strategic planning, deep reasoning

**Model resolution chain (first match wins; per-project levels retired by ADR-0061):**
1. Smart routing (if enabled per the derived rule, §27.2) → classifier picks tier → simple tier dispatches global `simple_tier_model` via the internal provider; complex tier falls through to 2
2. Global default model (`ai_settings.model`, from Settings → AI)
3. Error (no hardcoded fallback — user must configure)

**Scope:** Smart routing applies ONLY to user-facing chat responses. Background AI tasks use their own dedicated model settings and are NOT affected by smart routing. As of 2026-05-18, the dedicated settings in `ai_settings` are: `memory_search_llm_rerank_model` (Memory rerank — canonical key; the `rerank_model` key written by the v0.33.0 AI tab picker is retired per ADR-0037 §3) and `extraction_model` (Memory extraction — see memory-pipeline.md §5.4C, ADR-0038). The synthesis model named in ADR-0010 §5.5.3 ships under `synthesis_model` and follows the same pattern when synthesis lands.

### 27.2 Database Changes

> **RETIRED — per-project routing columns (ADR-0061, 2026-08-16):** The four `projects` columns `default_model`, `smart_routing_enabled`, `router_model`, `simple_tier_model` are retired. They remain physically in the schema (SQLite column drops require a table rebuild) but have zero production read or write sites after the ADR-0061 code fix, and the ADR-0061 one-time migration sets all four to NULL. Test fixtures declaring them are historical schema. No reader or writer may be reintroduced without a superseding ADR.

**AI settings (key-value) — live:**
- `simple_tier_model` → model ID. The single smart-routing configuration key: it is both the classifier model and the simple-tier response model (§27.6.1a). Written by the Settings → AI Smart Routing picker and the onboarding Smart Features step (ADR-0037 §3a); cleared by the Settings toggle-off path.
- Smart routing has **no stored enable key**. Enablement is derived: enabled iff global `simple_tier_model` is non-empty AND an internal provider is configured (ADR-0061 Decision 4). The display, the gate, and the data cannot disagree.

> **RETIRED `ai_settings` keys (ADR-0061):** `smart_routing_enabled` (never read by production code), `router_model`, `tier_1_model`, `tier_2_model`, `tier_3_model` (zero production references post-fix; `tier_1_model` was an orphaned-read legacy fallback). Deleted by the ADR-0061 migration; never reintroduce.

**Chat messages:**
- `chat_messages_local.model` column (already exists) — populated with the model that generated each response

### 27.3 UI Changes

Settings → AI is the single home for model selection across all roles (primary chat, smart routing, rerank, extraction, future synthesis). Feature behavior (toggles, sliders, thresholds) lives in the feature tab whose behavior it governs. Enable toggles for features that have a dedicated feature tab also live on that feature tab, not on the AI tab. See ADR-0037 for the architectural rule.

**Settings → AI tab structure:**

| Section | Pickers | Toggles / config |
|---|---|---|
| AI Connection | Primary provider + primary model (`model`) | Provider selector, API key, endpoint, test connection |
| Smart Features | Smart Routing Model (`simple_tier_model`), Rerank Model (`memory_search_llm_rerank_model`), Extraction Model (`extraction_model`, per ADR-0038) | Smart routing enabled, internal provider config |

Each picker writes a single canonical `ai_settings` key consumed by exactly one backend call site. Per ADR-0037 §3, decorative pickers (writing keys nothing reads) are not permitted; PR review verifies the round-trip from picker write to backend reader.

The "AI Connection" and "Smart Features" sections host model pickers and provider/routing configuration only. Per ADR-0037 §2a, enable toggles for features whose picker lives in the AI tab and that have a dedicated feature tab (e.g., memory reranking) live on the feature tab, not the AI tab.

**Cross-references from feature tabs to AI tab.** When a feature tab surfaces a backend behavior whose model setting lives in the AI tab, the feature tab displays the configured model name greyed-out + read-only with the inline pointer "Set in Settings → AI." The greyed display reads the canonical key — same source of truth, two surfaces. Per ADR-0037 §4.

Feature tabs never host model pickers, enable toggles for features whose picker lives in the AI tab, or any input that writes a model-role key. The cross-reference label is read-only display only — the only model-config surface a feature tab may present.

Current cross-references:

- **Memories tab → Rerank Model.** Greyed display of `memory_search_llm_rerank_model`. The "Enable Memory Reranking" toggle (`memory_search_llm_rerank_enabled`) and rerank-tuning sliders remain in Memories per the feature-behavior rule (memory-pipeline.md §5.15.6).

Future cross-references (e.g., when synthesis surfaces a configurable behavior) follow the same shape.

> **RETIRED — Project Settings → AI (ADR-0061, 2026-08-16):** The per-project routing UI formerly specified here (smart routing toggle, default-model dropdown, router-model + tier-mapping dropdowns) shipped in v0.25.1 (#384 Phase 2) and was removed in v0.32.3 (`8a7f6a7c`, "Smart routing is now global only"). This section survived the removal and described a surface that did not exist — the spec/code divergence that let #2517 / #2531 / #2533 accrue. Smart routing configuration is global-only: Settings → AI → Smart Features (`simple_tier_model`), per the table above. No per-project routing UI may be reintroduced without a superseding ADR (ADR-0061 Decision 1; ADR-0037 Risks).

**Chat UI:**
- Model label on each AI response (e.g., "sonnet-4" pill after tool indicators)
- "Analyzing..." indicator during classification (200-500ms before stream starts)

> **📌 Per-message model badges are immutable at stream time (2026-06-11, #1448):** The badge attached to an assistant message reflects the model that ACTUALLY ran for that specific message. It is set once at stream time from the value populated into `chat_messages_local.model` (§27.2) and is never updated retroactively — not on provider switch, not on model rename, not on app restart, not on any subsequent setting change. This is **load-bearing for smart routing**: different messages in the same conversation legitimately ran on different models (Tier 1 / Tier 2 / Tier 3 per §27.1, or historical per-project resolution — retired by ADR-0061; badges written under it remain valid audit records), and the badge is the audit trail of that decision. A future spec or implementation change that "fixes" stale badges by syncing them to the current provider/model would silently destroy the audit trail and break smart-routing introspection. The display invariants for current-provider state (Settings primary-model picker per §27.6.1c; AI Health Context Budget row per ai-integration.md §27.8 inv #11) apply to *current state* surfaces, NOT to per-message badges. The two surfaces have opposite refresh semantics and the distinction is deliberate. Cross-referenced from `docs/INVARIANTS.md` Smart Routing (§27) section for deletion-resistance.

> **📌 Why This Exists — Settings → AI as single home for model selection (2026-05-18, ADR-0037):** The v0.33.0 redesign moved model pickers into "Smart Features" without an explicit architectural rule for where pickers belong vs where feature behavior belongs. The unstated rule produced two violations: (1) a decorative picker (AI tab Rerank Model wrote `rerank_model`, a key the backend does not read) that silently disabled memory reranking from the AI tab surface for the entire v0.33.0+ window, and (2) a misplaced enable toggle (the AI tab's "Enable Memory Reranking" duplicates the canonical Memories tab toggle's role and creates the same dead-key shape's risk profile on a different surface). Pinning the architectural rule before adding extraction (ADR-0038) and synthesis (ADR-0010 §5.5.3) prevents the same shape recurring on each new picker.

### 27.4 Implementation Notes

**Classifier:** Haiku call before main response, <500ms timeout, conservative bias (tier up when uncertain)

**Streaming protocol:** Emit `classifying` event before classifier runs, `tier_selected` after, then normal `chunk`/`done` events

**Fallback:** On classifier timeout/error → use Tier 2 (Sonnet) as safe default

**Provider-aware model lists:** Fetch from OpenRouter `/models`, Ollama `/api/tags`, OpenClaw `/bridge/models`, or use hardcoded lists for Anthropic (no API endpoint)

### 27.5 Rollout Phases

**Phase 1 (v0.23.0):** Provider-aware model lists + populate `chat_messages_local.model` + model label UI

**Phase 2 (v0.24.0):** Project settings page + manual model selection + resolution chain

**Phase 3 (v0.25.0):** Smart routing classifier + tier mapping + streaming indicators

**Phase 4 (v0.26.0):** ✅ BUILT — OpenClaw provider model override via `x-openclaw-model` HTTP header. When using OpenClaw provider, Pane sends selected model as header (e.g., `x-openclaw-model: anthropic/claude-opus-4-6`). Header uses full provider/model format (NOT aliases). Only sent for user chat messages, not background operations (rerank, extraction, synthesis). See audit: `docs/audits/x-openclaw-model-integration-audit.md`

**Phase 5 (v0.27.0):** ✅ BUILT — Model Identity Separation (#384). The `ai_settings.model` field now stores provider model format (e.g., `anthropic/claude-opus-4-6`) instead of agent routing target (`openclaw:main`). This ensures:
- **Storage:** `ai_settings.model` always uses provider format (contains `/`)
- **Request body:** `model: "openclaw/main"` for agent routing (derived in `openrouter.rs`)
- **Request header:** `x-openclaw-model` uses stored provider model
- **Default:** When no model specified for OpenClaw provider, defaults to `anthropic/claude-opus-4-6`

**Phase 6 (future, optional):** Cost tracking dashboard

### 27.6 Dual-Provider Routing (§29 normative)

> **Status:** PARTIAL — classifier + simple-tier dispatch BUILT (v0.32.1, v0.32.2); rerank dispatch alignment landed via ADR-0037 (v0.65.41); extraction dispatch lands per ADR-0038 amendment 2026-05-19 + #1201; synthesis pending ADR-0010 §5.5.3.
> **Origin:** `docs/archive/smart-model-routing-spec.md` §29. This subsection is the live normative form; the archive is historical context.

Pane operates two provider channels. The **primary provider** carries chat completion only. For Tier 3 (OpenClaw) users this is the gateway, which intentionally translates the model field at `chat.rs:41` as part of the gateway's chat-routing product. The **internal provider** carries every background AI call — Smart Routing classifier and simple-tier dispatch, Memory rerank, Memory extraction, future synthesis. The internal provider must be direct (no gateway in the path), so that the model field survives end-to-end and the customer's per-feature model picks are honored.

> **Smart Features use their own provider setting, separate from the agent's chat provider; choose Venice there too if you want every AI call on Venice.** (#2988) The two dials — `ai_agents.runtime_type` for chat and `ai_settings.internal_provider` for Smart Features (§27.6.2) — are independently configured and can legitimately disagree; nothing in Pane currently surfaces that disagreement to the customer. A customer who sets their chat agent to Venice but never opens Settings → AI → Smart Features has their memory extraction, Smart Routing classification, and rerank calls still going wherever `internal_provider` already pointed (default: silently disabled per §27.6.3, or whatever provider was configured previously) — this line is the documentation of that fact, not a product fix for it.

**Primary-channel resolution is per-session (2026-08-16, ADR-0062/0063; normative home sessions.md §30.7.1):** chat dispatch resolves `session → agent → runtime` — phase 1 within the global mode, phase 2 via the agent's pinned runtime. **Nothing in this section changes:** routing scope stays global-only with the internal provider as its own standalone connection (ADR-0061 D5/D6), classification and simple-tier dispatch ride the internal connection regardless of any session's runtime, and the per-feature lookup rule is untouched.

Background AI features follow a **per-feature lookup rule** (ADR-0038 amendment 2026-05-19, ADR-0037 §3a). Each feature reads its own canonical model setting and gates on that setting alone. Null = feature silently disabled. No cross-feature fallback (extraction does not fall through to `simple_tier_model`; rerank does not fall through to anything). No fallback to primary (background features do not route through the gateway under any circumstance).

#### 27.6.1 Call-path matrix

| Feature | Call site | Model setting | Behavior on null |
|---|---|---|---|
| Chat completion (streaming) | `ai/chat.rs` streaming path | `model` (primary) | Chat disabled — user sees onboarding redirect |
| Smart Routing classifier | `classify_message_complexity()` | classifier model (via internal provider) | Smart routing disabled; chat falls through to primary (complex tier) |
| Smart Routing simple-tier dispatch | `resolve_model_for_message()` simple-tier branch | `simple_tier_model` | Smart routing disabled (treated as classifier failure → complex-tier path) |
| Memory rerank | `call_llm_for_rerank()` | `memory_search_llm_rerank_model` | Rerank disabled — search returns un-reranked results |
| Memory extraction | `extraction_worker.rs` (§5.4C) | `extraction_model` | **Extraction disabled — no memory write, no daily-log entry, no toast, no banner** |
| Memory synthesis (future) | TBD per ADR-0010 §5.5.3 | `synthesis_model` | TBD |

Gate behavior is uniform: null = silently disabled. No toast, no error banner, no log warning beyond a single info-level line at worker startup noting the feature is disabled because its model setting is null. The disabled state is visible to the customer in Settings → AI → Smart Features (picker is empty / shows disabled-state copy) and in the Memories tab (rerank cross-reference label reads "(none configured)").

> **fix(runtime) (#3120 PR 2, C6):** "Chat completion (streaming)" and "Smart Routing classifier/simple-tier dispatch" above described `ai_chat_stream`'s own interactive-turn call to `resolve_model_for_message` only — a scheduled job turn (`ai::scheduler::prepare_job_dispatch`) never called it at all before this fix, so an API agent with smart routing enabled got the cost-optimized simple-tier model for every interactive message but never for a job/wake/recurring/heartbeat turn. `prepare_job_dispatch` now calls the SAME `resolve_model_for_message` function, before its own `apply_budget` pass (so budgeting sees the EFFECTIVE model's context window, not the agent's configured one) — same gate, same classifier, same null-behavior (classifier/simple-tier failure falls through to the agent's own configured model, unchanged). `runtime.md` §36.3.3 is the job-turn call site; this file's call-path matrix above is unchanged in shape, only its scope widens to cover both turn origins.
>
> **fix(runtime) (#3155, C48):** C6 above made job turns resolve the SAME effective model/provider interactive turns do, but missed half the job — the CREDENTIAL. `prepare_job_dispatch` kept deriving `(api_key, openclaw_base_url)` from `dispatch_cfg.provider` (the agent's primary, configured runtime) BEFORE calling `resolve_model_for_message`, so a classifier-simple turn correctly switched `provider_for_streaming` to the `internal_provider` override but still streamed with the PRIMARY provider's stale credential — every classifier-simple job turn with Smart Routing's `internal_provider` configured got a `401 Missing Authentication header` (or worse, a different provider's key). The derivation now happens AFTER `resolved_provider` is known, sourced from it instead of `dispatch_cfg.provider` — the exact ordering `ai_chat_stream` already uses from `effective_provider`. C6's claim of "same gate" was true for model/provider selection, not for the credential sent; this PR closes that gap.

UI sublabel: 'Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.'

#### 27.6.1a Classifier model resolution detail

The "Smart Routing classifier" row above resolves its model from a single source: the global `simple_tier_model` in `ai_settings` — the customer's Smart Routing Model picker value. The same key supplies the simple-tier response model (the model that answers when the classifier says "simple"). One key, both roles.

If `simple_tier_model` is empty or unset, the classifier returns error and Smart Routing is treated as disabled per the matrix (chat falls through to primary, complex tier) — consistent with the derived-enablement contract (§27.2, ADR-0061 Decision 4), under which routing cannot be enabled while the key is empty. The chain runs entirely via the internal provider (per §27.6.2); the gateway is never in the classifier path.

> **RETIRED (ADR-0061, 2026-08-16):** The former three-level chain (`project.router_model` → `project.simple_tier_model` → global `simple_tier_model`) is retired. `project.router_model` was an orphaned read — zero write sites ever existed, so the top-priority level was unconfigurable fiction (#2533). `project.simple_tier_model` was live-but-invisible pre-v0.32.3 configuration; at retirement it also fed the simple-tier response model (session source-read 2026-08-16, correcting audit-2 §5's classifier-only description). A legacy `tier_1_model` fallback in the response-model path is retired with them. The forward-compatible per-project-picker language formerly in this subsection is superseded: per-context overrides, if ever wanted, belong to the per-session binding architecture and require a new ADR (ADR-0061 Decision 6; ADR-0037 Risks).

#### 27.6.1b Primary model field for OpenClaw users

The "Chat completion (streaming)" row above lists `ai_settings.model` as the primary model setting. For customers whose primary provider is OpenClaw, this field is a **display cache of OC's resolved model**, not an authoritative input to dispatch. The stored value drives Pane-side display surfaces (the Settings primary-model row, the AI Health Context Budget Model row per ai-integration.md §27.8 inv #11) and is kept in sync with OC's truth via the reverse-channel header contract specified in §27.6.1d. On the wire, Pane sends the `"openclaw"` sentinel (§27.6.1d) and the gateway resolves the real model from the OC agent registry; the value stored in `ai_settings.model` has no effect on which model handles the chat. This sub-section catalogs the supporting state and behavior:

- At onboarding, `ai_settings.model` is set to a hardcoded fallback value (`anthropic/claude-opus-4-6` at time of writing) when no model has been picked. The Settings.svelte primary-model picker is **read-only for OC users** — the customer cannot change `ai_settings.model` through the Settings UI. (Note: `docs/spec/sync.md` §22C.11A.1 specifies eventual removal of the model row from the Settings AI tab for OC users via #882; that removal has not yet shipped, so the picker remains visible-but-read-only in current code. This is the state #1448 surfaced. Once #882 ships and the picker is hidden, this rule remains in force — the customer still cannot change `ai_settings.model` through the Settings UI, the surface is simply no longer present.)
- At chat dispatch, the gateway rewrites the model field at the §29 agent-routing boundary to `"openclaw"`. OpenClaw's default agent answers the request regardless of what `ai_settings.model` held.
- The field is read on the request path but its value has no effect on which model handles the chat. The intentional gateway model-rewrite is part of OC's chat-routing product and is distinct from the extraction bug class closed by ADR-0038 amendment 2026-05-19 (extraction is a background AI call that must NOT travel via the gateway; chat is the only call that legitimately does).

The customer-facing tell is the log line `using user's chosen model: <model>`, which is misleading because OC users cannot choose. Log-line terminology cleanup is tracked in #1199 (Smart Features Setup); the spec authority for that cleanup is this sub-section.

#### 27.6.1c Settings primary-model picker reflects runtime-resolved state on provider save

The "Chat completion (streaming)" row from §27.6.1 names `ai_settings.model` as the primary model setting. For OC users, §27.6.1b establishes that `ai_settings.model` is decorative at dispatch — the gateway resolves the actual model from the OC default agent at chat time. The Settings primary-model picker that displays this value MUST reflect the gateway-resolved value after a successful primary-provider save, not the value held before the save.

The provider-switch sequence that surfaced this gap (issue #1448):

1. Customer's primary provider is OC. `ai_settings.model` holds the OC-resolved value from onboarding (or a prior switch-back). Picker is read-only ("Set in OpenClaw" per §6.5 onboarding model-step shape).
2. Customer switches primary to OpenRouter. `ai_settings.model` is overwritten with the OR model the customer picks. Picker becomes editable and shows the OR model.
3. Customer switches primary back to OC. The save codepath (e.g., `ai_configure_and_save`) calls into the OC agent-resolution path (e.g., `fetch_oc_agent_model`), resolves the OC default agent's model, writes the resolved value to `ai_settings.model`, and updates runtime `state.model`. The backend is now correct.
4. WITHOUT this invariant: the Settings picker continues to display the OR value from step 2 (or an empty/stale value), because the frontend picker state was not re-read from the now-updated backend state. The customer sees a wrong model in Settings while chat dispatches via the gateway-resolved correct one.

The invariant: on a successful primary-provider save, the Settings primary-model picker MUST re-read the post-save runtime state (e.g., `status.model` returned by the save codepath or fetched fresh after) so the displayed value matches what was actually resolved and persisted. The rule applies whenever the primary provider is OC (gateway-resolved), and is a no-op when the provider is direct (the picker's pre-save state already matches the saved state). The model-list backing the picker (`providerModels` or equivalent) is invalidated by the same save — picker options correspond to the new provider's catalog, not the prior provider's.

Implementation may also choose to refresh this state on `loadAiConfig` (mount) and on `handleProviderChange` (provider-dropdown change) as defense-in-depth, but the post-save read is the load-bearing path — that's the path that produced the bug and the path the invariant targets. (#1448)

> Note: Settings `loadAiConfig` does NOT auto-populate `ai_settings.model` from the gateway model catalog on the OC path; the correct value arrives via §27.6.1c (save-time `fetch_oc_agent_model`) and §27.6.1d (stream-time `X-OC-Resolved-Model` header). (#935)

#### 27.6.1d Reverse-channel model sync — sentinel on wire, resolved model in response header

§27.6.1c covers the configuration-event case (picker refresh on provider save). This sub-section covers the **ambient-drift case**: keeping `ai_settings.model` in sync with OC's truth on every OC chat stream, so that edits to `openclaw.json` outside Pane (or any other path that changes OC's resolved model without a Pane-side configuration event) propagate to Pane's cache and display.

**Background — the bug this contract closes.** Pre-#1453, `ai_settings.model` was populated only at provider-save time via `fetch_oc_agent_model` (the OC agent-resolution call invoked by `ai_configure_and_save`). No other refresh path existed. When OC's `openclaw.json` was edited between Pane saves — directly by the customer, or by any OC-side tooling — Pane's cache drifted from OC's truth without any signal. OC has always owned the model (the gateway has always resolved from `agent_registry` per §22C; Pane has never chosen or guessed the OC model); the bug was specifically the missing continuous-refresh path for the display cache.

**The contract has three parts.**

**1. Pane → gateway: sentinel on wire.** On the OC chat path (primary provider is OC), Pane sends `body["model"] = "openclaw"` in the chat-completions request body. This is the sentinel value the gateway rewrites the field to anyway (per §27.6.1b bullet 2 and the `chat.rs` rewrite that lives at the §29 agent-routing boundary). The catalog ID Pane previously sent on this path (e.g., `anthropic/claude-opus-4-6`) was a debug hint only — the gateway has always resolved the real model from `agent_registry` and rewrites the body field regardless of what Pane sent. The sentinel is the honest version of the same wire shape; no semantic change in dispatch behavior, just wire-format hygiene that pairs with the reverse-channel header introduced in part 2. (The legacy outbound `x-openclaw-model` header carrying Pane's catalog ID is unaffected by this rule — see §22C cross-ref for that header's status as gateway-side debug-hint-only.)

**2. Gateway → Pane: resolved model in response header.** The gateway returns the OC-resolved model in the `X-OC-Resolved-Model` response header on `/v1/chat/completions` responses (both streaming and non-streaming). The header value is the model identifier the gateway resolved from `agent_registry.get_agents()` at request time — already computed for PARAM_COMPAT, no additional lookup. See `docs/spec/sync.md` §22C.6.7 for the gateway-side emission contract.

**3. Pane cache update on stream completion.** Pane reads the `X-OC-Resolved-Model` response header and, on **successful stream completion** (the stream received `[DONE]` without error), compares the header value to the stored `ai_settings.model`. If they differ, Pane writes the header value to `ai_settings.model` and refreshes the runtime `state.model`. This is the continuous-refresh channel the OC model cache was missing — pre-#1453, the cache was populated only at provider-save time via `fetch_oc_agent_model` with no other refresh path; the reverse-channel header on every OC chat stream is the missing refresh channel, so the cache stays in sync with OC's truth continuously, not just at configuration events.

**Behavior on missing header (older gateway version, response error, non-OC chat path).** If the response does not include `X-OC-Resolved-Model`, Pane no-ops — it keeps the last-known `ai_settings.model` value and does NOT crash, error, or warn. This covers:
- Pre-contract gateway versions (older than the gateway PR that emits the header).
- Non-streaming responses that may not include the header due to non-streaming code path differences.
- BYOK chat paths (OpenRouter, Anthropic, Ollama direct) — no gateway in the path, no header expected, no-op is correct.
- Stream errors — Pane MUST NOT update the cache from a header on a failed stream.

**Composition with §27.6.1c.** Both §27.6.1c (configuration-event picker refresh) and §27.6.1d (ambient-drift cache update) target the same underlying store (`ai_settings.model` and runtime `state.model`). They compose without conflict: §27.6.1c handles the synchronous-with-save case; §27.6.1d handles the always-on case. After both are in force, any of the following correctly propagates OC's resolved model to Pane's display:
- Customer renames a model in `openclaw.json`, then sends a chat — §27.6.1d updates on stream completion; the next picker render reads the updated value.
- Customer switches primary provider to OC via Settings save — §27.6.1c reads `status.model` from the save codepath into the picker.
- Customer's first chat after upgrading to a gateway version that emits the header — §27.6.1d updates on stream completion; subsequent UI surfaces see the synced value.

**Not a dispatch input.** The cache value (`ai_settings.model` when provider is OC) drives Pane-side display only — it is never used to determine which model handles a chat. Dispatch always goes through the gateway, which always resolves from `agent_registry`. The cache and the dispatch path are decoupled by design; §27.6.1d keeps the cache HONEST about that decoupling. This decoupling is the architectural invariant locked in `docs/INVARIANTS.md` Smart Routing (§27). (#1453)

**Status:** BUILT in #1457.

> **📌 Why §27.6.1a + §27.6.1b exist (2026-05-20, Reed Addendum 3):** Addendum 3 traced smart routing end-to-end and confirmed two spec-clarity gaps in §27.6 as landed in PR #1204. Neither expands §27.6's normative scope — the classifier chain has been live since v0.32.1, and the OC primary-model decorative state has been live since onboarding shipped. The spec just didn't name them. Calling out both prevents future audits from re-tracing the same paths and gives #1199's log-line cleanup a spec anchor to cite.

#### 27.6.2 Internal-provider resolution chain
> **Venice row + Step 2 fallback:** BUILT (PR #2795, ADR-0065 Supplement S1)

The internal provider is resolved from `ai_settings` via `get_internal_provider(db)`. The internal provider dropdown offers every configured provider except those excluded by a capability gate (ADR-0065 S1). OpenClaw may serve as fallback per existing behavior. Currently supported providers:

| Provider | `internal_provider` value | Required keys | Endpoint | Auth header |
|---|---|---|---|---|
| OpenRouter | `openrouter` | `internal_api_key` | `https://openrouter.ai/api/v1` | `Authorization: Bearer <internal_api_key>` |
| Anthropic | `anthropic` | `internal_api_key` | `https://api.anthropic.com/v1` | `x-api-key: <internal_api_key>` + `anthropic-version: 2023-06-01` |
| Ollama | `ollama` | `internal_endpoint` (default `http://localhost:11434`) | `<internal_endpoint>` | (none — local) |
| Venice | `venice` | keychain only, account `ai_venice_key` (no `internal_api_key` fallback — ADR-0065 D4) | `https://api.venice.ai/api/v1` | `Authorization: Bearer <key>` |

> New runtimes joining this table add a row per ADR-0065 S1's universal-offering rule — there is no separate spec gate to satisfy beyond a row here and the corresponding hardcoded dispatch arm; see #2795 (Venice's row above) as the worked example.

**Venice's system prompt on this path (R1, #2795):** every internal-provider dispatch of `AiProvider::Venice` — extraction, Smart Routing classification, and Rerank alike — sends `venice_parameters.include_venice_system_prompt: false` unconditionally, regardless of the customer's install-wide "Use Venice system prompt" toggle (ADR-0065 D3, `venice_system_prompt_enabled`). D3's toggle governs the primary-provider **chat** path, where Venice's injected prompt competes with an agent's own SOUL.md/IDENTITY.md/USER.md documents for a human reader's turn. Extraction, classification, and rerank are background machinery with no user-facing conversational turn for Venice's prompt to steer — there is no agent persona for it to compete with, so the toggle's chat-scoped rationale does not transfer. This matches the existing `call_llm_from_config` background-worker precedent (`ai::provider`, synthesis/consolidation dispatch), which already hardcodes `false` for the same reason.

Resolution outcomes (first match wins):

1. `internal_provider` + required keys for that provider → dispatch direct.
2. `internal_provider` set but required keys missing → all background AI features disabled (treated as "no internal provider configured"). The Settings → AI → Smart Features section surfaces the missing-key state inline; onboarding §6.4 prevents this state at first run.
3. `internal_provider` null → all background AI features disabled. No fallback to primary. The customer has not opted into Smart Features.
4. `internal_provider` unset (Tier 1/2 back-compat fallback) and the default agent's `runtime_type` is a key-bearing provider (`openrouter` / `anthropic` / `venice`) with a resolvable key → that provider's key is reused for the internal path (Step 2 fallback, `ai::provider`). Venice-primary customers resolve through this arm exactly like OpenRouter/Anthropic-primary customers do — R2 (#2795) closed a gap where this arm previously hard-errored (`Err("Unsupported provider for internal calls: venice")`) for Venice-primary installs with no `internal_provider` row, the only one of the four key-bearing/local runtimes that fell through to the generic error instead of resolving.

**Diagnostic logging security:** `get_ai_setting()` and `set_ai_setting()` emit `[SETTINGS-LOAD]` / `[SETTINGS-SAVE]` lines to stderr for `internal_provider` and `internal_api_key` keys. The `internal_api_key` value is always masked to `***` in log output; the actual value is returned/stored unmodified. (v0.65.53, PR #1243)

Ollama-specific note: Ollama is a local endpoint with no auth header. The `internal_endpoint` key holds the full URL including the port (`http://localhost:11434` is the default). If the customer runs Ollama on a non-default endpoint, they configure it during onboarding §6.4 or via Settings → AI → Smart Features. `internal_api_key` is unused for Ollama.

#### 27.6.3 Gate behavior on null

The disabled state is **silent**:

- No error banner in the chat UI when a background feature is disabled. Chat works (via primary); the background feature simply doesn't run.
- No toast on the first extraction skip. The customer sees the absence of memory growth in the Memories tab, not a notification on every chat turn.
- No log warning beyond a single startup info line per worker (e.g., `[EXTRACTION] disabled — extraction_model is null`). Per-call log noise on a disabled feature is forbidden.
- **Settings → AI → Smart Features section surfaces the configuration state.** Pickers show their configured value or an empty/disabled state; this is the customer's path to enable a feature post-onboarding.
- **Memories tab rerank cross-reference label** reads "(none configured)" when `memory_search_llm_rerank_model` is null, per ADR-0037 §4.

The silent-disable model is load-bearing for cost: a customer who declines Smart Features at onboarding and never sets it up later should pay $0 for background AI tasks. Any error/banner/retry on every chat turn would push the customer to "fix" a non-bug, and would create surface area for the kind of regressions Phase 1 is closing.

#### 27.6.4 Cross-references

- **ADR-0038 (amended 2026-05-19)** — extraction model routing; §5.4C is the per-feature resolution detail
- **ADR-0037 (amended 2026-05-19) §3a** — onboarding pickers as a documented exception
- **memory-pipeline.md §5.4C** — extraction model resolution (per-feature lookup, no fallback)
- **memory-pipeline.md §5.15.6** — rerank configuration; cross-reference label semantics
- **onboarding.md §6.4** — Smart Features onboarding step (PR #1202)
- **`docs/archive/smart-model-routing-spec.md` §29** — historical origin; do not cite as live spec

> **📌 Why §27.6 was a stub:** Pre-2026-05-19, dual-provider routing was scoped as a Tier-3-OpenClaw-only concern, and the live spec deferred to the archived design doc. The Phase 1 product decision (2026-05-19) pulled three things into Pane's load-bearing path: extraction must route direct (not via gateway); three internal providers must be accepted (not just OR); per-feature lookup with no cross-feature fallback. With these locked, §27.6 became the source-of-truth for runtime behavior — too important to live in an archive. Reed audit 2026-05-19 (Addendum 2) was the empirical trigger; the architectural surface was already drifting.

---


#### 27.6.5 Greeting message bypass (#1241, v0.65.54)

> **Status: OBSOLETE as of #1541.** This bypass no longer has a subject. Both the post-onboarding greeting and the new project greeting are **fixed templates, not model calls** — nothing is dispatched, so there is no routing decision to bypass. See onboarding.md "Post-onboarding greeting (#1223)" and "Project greeting (#1541)", and the ADR-0039 supplement (2026-07-15).

The greeting no longer sets `greeting: true` for routing purposes because it does not call `ai_chat_stream` for generation. Any residual `greeting: true` handling in the routing path has no live consumer and carries no normative behavior; it may be removed in the code PR.

*Historical (pre-#1541):* the greeting was a model call routed to the user's primary model (project `default_model` → global `ai_settings.model`), skipping the classifier and simple-tier dispatch, to avoid a first-impression model mismatch. Retained here as history only.

#### 27.6.6 Classifier pattern coverage is generated from the tool registry; the prompt is tier-aware (#3053 PR 4 item C)
> **Status:** BUILT. See `docs/audits/2026-10-21-e8-simple-tier-repro.md` for the measured reproduction this section fixes, and ai-integration.md §4.6.2 for the companion prompt/wire-consistency fix.

**The bug this closes.** A live pipeline run (classifier → routing → prompt assembly → tool selection, real functions, only the outbound classifier LLM socket mocked) against "go home" and "star this project" showed: the classifier misjudged both as trivially simple (its hardcoded `tool_patterns` array had zero coverage for either trigger word — "home"/"star" were simply absent), smart routing selected the simple tier, 0 tools were sent on the wire, but the system prompt — built tier-blind, see ai-integration.md §4.6.2 — still listed all 39 tools including `navigate_home`/`star_project` by name. A simple-tier model told it has a tool it was never given can respond "Done, I've starred that!" with no tool call; nothing downstream checks response text against actions actually taken, so the false claim reaches the customer verbatim.

**Fix 1 — `tool_patterns` is generated from the live registry, not hand-maintained.** `ai::chat`'s `generate_tool_patterns_from_registry()` replaces the hardcoded array. It reads `tools::get_pane_tools_for_runtime("anthropic")` (the richest/superset runtime) and bridges each tool's `snake_case` name to trigger words by splitting on `_` and keeping tokens of 3+ characters (drops connector noise like "to"/"an"; every token this fix depends on — "set", "star", "home", "filter", "status", "priority", "cells", "kanban", "tasks", "projects", "view", "navigate", "show", "all" — clears that bar). Because it is computed from the live registry on every call rather than copied by hand, a newly-registered tool is covered automatically. A small, explicitly-separate hand-written array (`non_tool_name_bypass_patterns()`) covers two categories NOT derivable from any tool's own name — synonym verbs ("save"/"remember" for note creation, "open"/"fetch" for navigate/read) and temporal/content-reference phrases ("yesterday", month names, "daily log") — and is explicitly exempt from the registry-sync guarantee below, since it names no specific tool.

**Residual limitation, stated plainly:** a pure name-token generator cannot catch a natural phrasing that shares no word with the tool's own name. Two of the ten tools the audit found uncovered — `set_status` ("mark this task done") and `set_priority` ("make this urgent") — have natural phrasings that contain neither the tool's name tokens nor any synonym currently in the hand-written list. The audit's own stated root cause ("'set' isn't in the array at all") is fixed — `set_status`/`set_priority` ARE covered for any `set`-containing phrasing ("set the status to done", "set the priority to high") — but a full synonym table closing every colloquial phrasing is out of scope for this PR.

**Sync test.** `ai::tools`'s `pr3053_item_c_tier_aware_tools_tests::test_every_registered_tool_has_surviving_generated_token` iterates the LIVE registry (not a frozen snapshot) and asserts every tool has at least one surviving generated token from its own name. This fails loudly — naming the exact tool and its token list — if a future change to the length filter (or any other filtering logic) ever swallows every token of some tool's name, the exact silent-zero-coverage shape the audit found for the old hardcoded array.

**Fix 2 — the long-conversation classifier bypass is reachable.** `classify_message_complexity`'s `context_messages.len() > 10` bypass (unchanged logic) was dead code because its one real caller (`ai_chat_stream`, inside `resolve_model_for_message`'s call site) built `context_messages` via `messages.iter().rev().take(3).rev()` — unconditionally capping it at 3 before the classifier ever saw it, so `.len()` could never exceed 3 regardless of true conversation length. Fixed at the call site (`context_messages = messages.clone()`), not by lowering the `> 10` threshold: the threshold already expresses the correct policy (bypass when the REAL conversation is long), so the bug was in what was being measured, not the number it's measured against. Lowering the threshold to fit a 3-item cap (e.g. `> 2`) would have bypassed the classifier for nearly every message past the first couple of turns in ANY conversation — a far larger blast radius than fixing the one call site. This call-site slice feeds TWO things: the bypass check above, and the classifier's own prompt-context window (`classify_message_complexity`'s internal `context_str = context_messages.iter().rev().take(3).rev()...`, unchanged) — the internal windowing already re-slices to the last 3 messages for the actual text sent to the classifier model, so per-request classification INPUT TEXT for conversations under 10 messages is unaffected by this fix; only the previously-dead bypass decision now sees accurate data.
