# ADR-0065: Venice as a Pinnable Runtime

*Line citations predate the #3097 module split.*

**Status:** Proposed
**Date:** 2026-08-29
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0063 D1 (exercises the reserved `'venice'` value in the `runtime_type` domain), ADR-0064 D3 (exercises the explicit runtime opt-in for web tools), ADR-0044 (one new keychain account, same contract)
**Related:** #2455 (this ADR's issue), #2734 (per-model capability flags; Venice-specific slice resolved here, general question left open), ADR-0046 / §4.5.8 (text-fallback parser; unchanged), docs/audits/2026-08-06-venice-ai-research.md (Reed research), docs/audits/2026-08-29-venice-spec-source-read.md @ 96958857 (source census)

## Context

Venice is the next runtime in the ratified provider order (binding, runtime-per-agent, Venice, Hermes; Matt 2026-08-16). ADR-0063 D1 reserved `'venice'` as a future `runtime_type` value and declared it out of scope for that ADR. ADR-0064 D3 made the web-tools runtime allowlist an explicit opt-in so that a new runtime is a decision rather than an inheritance. This ADR is both decisions.

Venice speaks the OpenAI chat-completions wire format at `https://api.venice.ai/api/v1` with Bearer auth (research §1, §3). Two Venice-specific behaviors are load-bearing and must be carried by spec, not left to the build: Venice appends its own system prompt to every request unless `venice_parameters.include_venice_system_prompt: false` is sent (research §3), and Venice blocks a key for 30 seconds after more than 20 failed requests in 30 seconds (research §7). Venice exposes a per-model privacy tier and capabilities on `/models` (research §4). Venice's privacy claims are vendor-stated and unaudited, and its Anonymized tier still exposes prompt content to the upstream frontier lab (research §4, §7).

Strategy ratified by Matt 2026-08-29: Venice ships like OpenRouter. A provider option in + New Provider, a card matching the OpenRouter card plus one toggle, keychain key, live `/models` picker relaying Venice's own tier labels, web tools allowlisted in the same PR, Venice's built-in search flags ignored, retry logic that cannot trip the abuse block, and no privacy claims of Pane's own anywhere in product.

Source facts this ADR relies on (source-read @ 96958857):

- `AiProvider` is a four-variant enum (`Anthropic { api_key }`, `OpenRouter { api_key }`, `Ollama { endpoint }`, `OpenClaw { base_url, token }`). `build_provider_for_runtime_type_from_snapshot` errors on any other `runtime_type` (runtime_resolution.rs:112-160).
- The OpenRouter base URL is a constant, `OPENROUTER_API_BASE` (openrouter.rs:11), not an `ai_settings` key. `stream_openrouter` takes `base_url: Option<&str>`; `None` means OpenRouter, `Some(...)` is the OpenClaw gateway arm today.
- Two shipped behaviors key on that `Some(base_url)` shape as the OpenClaw signal: the §4A.2.4 temperature policy, and §30.7.2's session hint (passed only on OC arms).
- Request bodies in `stream_openrouter_inner` (openrouter.rs:568) and `chat_with_tools_inner` (openrouter.rs:242) carry standard OpenAI fields only; no `venice_parameters` object exists.
- `apply_openrouter_attribution` is gated on `base_url.contains("openrouter")` (openrouter.rs:285, 628); a Venice URL never triggers it.
- No 429 handling exists on the OpenAI-shaped path; both retry wrappers retry on parse/stream errors only.
- `provider_keychain_account` matches `"anthropic" | "openrouter"` only (credential_migration.rs:97); `read_provider_key_from_snapshot` falls back to plaintext `ai_settings.api_key` pre-migration. `read_brave_key` is keychain-only with no migration branch (the precedent for a post-ADR-0044 key).
- `{runtime}_configured_at` is written by `format!` in the `"anthropic" | "openrouter"` arm of `ai_configure_and_save` (commands.rs:4203); disconnect deletes are three hardcoded statements (commands.rs:4710-4725).
- `connected_runtime_providers` enumerates `["openclaw", "openrouter", "anthropic", "ollama"]` (commands.rs:20038); §28.4's two-phase loop covers the same four.
- `WEB_TOOL_RUNTIME_ALLOWLIST` is `["anthropic", "openrouter", "ollama"]` (tools.rs:476); the unknown-runtime test at tools.rs:576 uses the literal `"venice"` as its unknown value.
- `resolve_runtime_type_for_upsert` accepts exactly four strings (agents.rs:493). No CHECK constraint exists on `ai_agents.runtime_type`; the domain lives in Rust.
- `ModelInfo` is `{ id, name, provider, context_length, supported_parameters }`; `ModelRegistry` keys are `"{provider}:{model_id}"`. `SearchableModelSelect` takes `{ id, name, provider }`.
- `RUNTIME_LABELS` in `AgentCreateOverlay.svelte` has three entries; `ProviderConnectionsPanel` renders one card per `runtime_type` returned by `ai_list_runtime_connections`.

## Decision

### D1. `'venice'` joins the `runtime_type` domain; Venice is its own `AiProvider` variant

`runtime_type` domain becomes `'openclaw' | 'openrouter' | 'anthropic' | 'ollama' | 'venice'`. Venice agents are local agents: `runtime_ref` is NULL, `model` is required at creation (ADR-0063 D1), and they go through the existing local creation path with `runtime_type = 'venice'` supplied explicitly. Venice is never ambiently inferred by `resolve_runtime_type_for_new_local_agent`; the only way an agent becomes Venice-bound is an explicit `'venice'` at creation.

`AiProvider` gains a fifth variant, `Venice { api_key, include_system_prompt: bool }`. Venice dispatches through the existing OpenAI-shaped functions in `openrouter.rs` with `base_url = Some(VENICE_API_BASE)` where `VENICE_API_BASE = "https://api.venice.ai/api/v1"` is a constant, matching `OPENROUTER_API_BASE`'s shape. No `ai_settings` key holds the Venice base URL.

**Venice is not OpenRouter-with-a-base-URL.** Every match arm that today distinguishes OpenRouter from OpenClaw by `base_url.is_some()` must distinguish by variant instead, or Venice inherits OpenClaw behavior it must not have: Known sites (build-scope census, docs/audits/2026-08-29-venice-build-scope.md §3): the §4A.2.4 temperature policy, the §30.7.2 session hint, `openrouter.rs:158` / `:489` (gateway client selection — SPKI-pins to the stored OC cert, or `danger_accept_invalid_certs(true)` when no cert is stored; Venice must never route through `build_gateway_client()`), and `openrouter.rs:604-605` (model sentinel — unconditionally overwrites `body["model"]` to `"openclaw"` on any `base_url.is_some()`). The full census classifies ten predicates across `openrouter.rs` and confirms `commands.rs` dispatch is already variant-keyed. This is the one place the "drop-in" framing in research §6 is wrong for Pane specifically.

**Not a CRR or constraint change.** `ai_agents` INSERT/UPDATE behavior is unchanged; `idx_ai_agents_runtime_unique(runtime_type, runtime_ref)` cannot collide on Venice rows because `runtime_ref` is NULL and NULLs are distinct. No constraint table is required (ADR-0063's table stands).

### D2. Venice request shape: `venice_parameters` carries exactly one field; Venice search flags are never sent

On the Venice arm only, the request body gains:

```json
"venice_parameters": { "include_venice_system_prompt": <bool> }
```

where `<bool>` is the agent's runtime setting from D3. No other `venice_parameters` field is ever sent. In particular `enable_web_search`, `enable_web_scraping`, `enable_x_search`, and `character_slug` are never set (locked, Matt 2026-08-29). Rationale: Venice's built-in search would return page content into the model's context outside Pane's web tools, bypassing the §4.6.3 taint mechanism, the §4.6.5 SSRF guard, the §4.6.9 extraction exclusion, and the user-visible "Tools used" log. Pane's own web tools are the only browsing path on every direct runtime; Venice is no exception.

The injection site is the `let mut body = json!({...})` assembly in both `stream_openrouter_inner` and `chat_with_tools_inner`, keyed on the variant, never on the URL string. No `venice_parameters` object is ever attached to an OpenRouter or OpenClaw request.

### D3. "Use Venice system prompt" defaults OFF; stored as `ai_settings.venice_system_prompt_enabled`

The card toggle defaults to off, meaning Pane sends `include_venice_system_prompt: false`.

Why off, not Venice's native on: the agent's SOUL.md, IDENTITY.md, USER.md and the Layer 2 rules are the system prompt. Venice's default prompt is undisclosed third-party text appended beneath them, written to steer models toward Venice's "uncensored" posture (research §4). Pane has never adopted a vendor's ambient prompt on any runtime; OpenRouter and Anthropic send only what Pane assembles. Off keeps the agent's documents authoritative and keeps the §4.6.3 system-prompt rule the last word in the prompt. On is offered for users who want Venice's native behavior, and it is a per-install preference, not per agent.

Storage: `ai_settings.venice_system_prompt_enabled`, values `"1"` / `"0"`, absent = off. Name matches the existing `{feature}_enabled` boolean pattern (`link_previews_enabled`, `memory_search_llm_rerank_enabled`). Writer: the Venice card, through `set_ai_setting` directly (the frontend-write precedent of `internal_configured_at` and `gateway_paired_at`, §28.3). Reader: `RuntimeSettingsSnapshot::capture` adds the key, and `build_provider_for_runtime_type_from_snapshot`'s Venice arm folds it into the variant. Migration disposition (ADR-0061 D7): none; absent is a valid state; the key is not deleted on disconnect (it is a preference with a live reader, not connection state).

### D4. Key in keychain as `ai_venice_key`, keychain-only read

New constant `KEYCHAIN_ACCOUNT_VENICE_KEY = "ai_venice_key"`, added to `ALL_AI_KEYCHAIN_ACCOUNTS` so backup export carries it (ADR-0044 Decision 4). `provider_keychain_account` gains the `"venice"` arm so `write_provider_key`, `read_provider_key`, and the `"anthropic" | "openrouter"` arm of `ai_configure_and_save` (which writes `{runtime}_configured_at` by `format!`, yielding `venice_configured_at` for free) all work unchanged.

One divergence, mechanical: `read_provider_key_from_snapshot`'s pre-migration plaintext fallback to `ai_settings.api_key` must never apply to Venice. The key postdates ADR-0044; there is no plaintext era. The Venice arm returns None on keychain miss regardless of the migration flag, the same posture as `read_brave_key`. `ai_disconnect_runtime` gains `DELETE ... WHERE key = 'venice_configured_at'` beside the three existing deletes and deletes the keychain account.

### D5. Live `/models`; Venice's tier label relayed verbatim; Pane says nothing about privacy

`fetch_venice_models` mirrors `fetch_openrouter_models_at` against `{VENICE_API_BASE}/models` and sets `provider: "venice"`, so `ModelRegistry` keys become `"venice:{model_id}"` and `SearchableModelSelect`'s `provider` filter works with no change. `context_length` maps from Venice's per-model context field.

`ModelInfo` gains one additive field, `tier_label: Option<String>` (serde default, None for every other provider). For Venice it holds the string Venice returns in the model's privacy field, unmodified: no mapping table, no Pane vocabulary, no capitalization or rewording. The picker renders it as a small trailing tag on the row. When the field is absent or empty, no tag renders; Pane never substitutes "Unknown" or any other word. The field names on Venice's payload (research names `model_spec.privacy`; capabilities carry E2EE and TEE booleans) are verified by Reed against a live `/models` response at scope time and recorded in the scope; the spec fixes the contract, not the JSON path.

Pane makes no privacy claims of its own for Venice anywhere in product: not on the card, not in the picker, not in the New Provider modal, not in error copy. The tag is Venice's word, attributed by placement (it sits on Venice's models only). Locked, Matt 2026-08-29; consistent with the standing rule that privacy claims are conditional and that "fully private" is said of Ollama only.

### D6. Web tools: `venice` joins `WEB_TOOL_RUNTIME_ALLOWLIST` in the same PR; both gates apply

`WEB_TOOL_RUNTIME_ALLOWLIST` becomes `["anthropic", "openrouter", "ollama", "venice"]`. This is ADR-0064 D3's opt-in exercised, not amended. Both enforcement points (§4.6.2 registry, §4.6.2 dispatch gate) already key on `runtime_type` and need no Venice-specific code; the allowlist edit is the whole change. The unknown-runtime test at tools.rs:576 must switch its literal to a string that is not a runtime (e.g. `"nonesuch"`), and a positive test asserting `"venice"` receives both web tools joins §4.6.10 item 8.

Reed's scope must verify the gate with a real Venice call, not a unit test: a Venice-bound session, `[WEB-GATE] runtime_type=venice web_tools=offered` in the log, a `web_fetch` executed, and a `[WEB-TAINT]` refusal of `edit_note` in the following iteration. Execution-log evidence, not intent.

Venice's own search stays off (D2), so the taint and extraction mechanisms see every byte of external content Venice models receive.

### D7. 429 backoff that honors Venice's reset headers and cannot trip the abuse block

On the Venice arm, HTTP 429 is classified `venice_rate_limited` and retried with backoff: if `retry-after` or `x-ratelimit-reset-requests` is present and parses to a duration, wait that long (capped at 30 s); otherwise 1 s, 2 s, 4 s. Maximum three attempts per dispatch, then surface the §4.3.4 copy. 401, 403, and any insufficient-balance status are hard fails with no retry (same posture as §4.3.2's billing rule and INVARIANTS "Billing errors never enter retry/reconnect loops"). Reed's scope verifies Venice's actual insufficient-balance status code with a live call; research §5 does not record it.

Why this cannot trip the block: the block fires on more than 20 failed requests in 30 s. Hard fails produce one failed request per dispatch. 429 produces at most three per dispatch spaced at least 1 s apart, and a user cannot send more than a handful of dispatches in 30 s. Model-list fetches are single requests. The arithmetic is recorded here so a future "make retries more aggressive" change has to re-derive it.

Venice's `x-venice-model-deprecation-warning` header is logged as `[VENICE-DEPRECATION] model={} warning={}` when present and is not surfaced in UI in this PR (see D9).

### D8. Card, modal, picker, roster: Venice appears wherever OpenRouter does, plus one toggle

- + New Provider lists Venice beside Anthropic, OpenRouter, Ollama, OpenClaw. `NewProviderModal`'s API-provider configure step (key field) applies; no base URL field (the URL is a constant). The configure step carries a "Get a key" link to `https://venice.ai/settings/api`, matching the OpenRouter configure step.
- `ProviderConnectionsPanel` renders cards only for configured providers (corrected 2026-08-30, #2777 venice live-test scoping — supersedes the original "renders five cards, always, connected or not (ADR-0063 D8 'always all runtimes' extended)" text below, which shipped as written and was found to be a design decision needing correction, not a bug, once live-tested). A card appears once `configured_this_install` is true for that runtime; "+ New Provider" is the sole entry point for an unconfigured provider and remains always visible. See ADR-0063 D8's own note for the cross-reference back to this correction.</br>&nbsp;&nbsp;~~`ProviderConnectionsPanel` renders five cards, always, connected or not (ADR-0063 D8 "always all runtimes" extended).~~ (struck 2026-08-30, kept for history)
- The Venice card is the OpenRouter card plus one toggle, "Use Venice system prompt" (D3). Same three-state color logic (§28.3) via `venice_configured_at`. No pricing text, no privacy copy. Links on the card: none (locked, Matt 2026-08-29).
- `AgentCreateOverlay`'s `RUNTIME_LABELS` gains `venice: 'Venice'`; the connected-runtime filter already admits any non-openclaw connected runtime.
- The Agents tab groups Venice agents under a Venice heading, same as the other four runtimes.
- The legacy-unpinned one-time picker (ADR-0063 D4.3) lists Venice if connected; nothing special.
- Validation: `validate_venice_key` is `GET {VENICE_API_BASE}/api_keys/rate_limits` with Bearer auth; 200 = valid, 401 = rejected. `/models` is unauthenticated (200 without a key, scope 2026-08-29) and cannot distinguish valid from invalid keys. §28.2's table gains the row; the §28.2 INVARIANT (no green without a real validation result) applies unchanged.

Out of this PR, recorded so it is not entry-point blindness: onboarding §6.2's provider picker stays four cards; Smart Features' internal-provider dropdown does not offer Venice (superseded — see Supplement S1 below: Smart Features offering Venice shipped as #2795, the first exercise of S1's universal-offering rule, not a further amendment to this ADR); embeddings engines do not offer Venice. Each is a separate decision if wanted.

### D9. Deprecation header surfacing deferred to a follow-up issue

Logged now (D7), not shown. The right surface is per-agent (the deprecating model is `ai_agents.model` on specific agents), which is an agent-card design question Claude Design has not touched, and no header-reading path exists on the OpenAI-shaped response today. Brit files the follow-up; the log line gives evidence in the meantime.

### D10. ADR structure: one ADR

A separate supplement to ADR-0064 for the allowlist change would be a one-line document whose entire content is "Venice opts in." D3 of that ADR was written to make this exact decision routine. Recording it as D6 here, with the Supplements line pointing at ADR-0064 D3, keeps the decision findable from both directions without a filler ADR.

### D11. #2734 slice

Resolved here for Venice: the runtime-level web-tools capability (D6). Left to #2734 in full: per-model native-tool-calling flags, gating the §4.5.8 text-fallback parser on capability, marking a model tool-incapable in the picker, and any capture of Venice's per-model `supportsFunctionCalling` into `ModelInfo`. This ADR deliberately does not add a capability field to `ModelInfo` beyond `tier_label`; a captured-but-unread capability flag would be the dark-data shape ADR-0061 D7 rejects for settings. Venice's `/models` is noted as a ready source for #2734 when it is scoped.

## Consequences

**Positive.** A fifth pinnable runtime with roughly forty models, including proxied frontier models, behind one key, using the existing OpenAI-shaped path, the existing card and picker components, and the existing web-tools gates. Every Venice-specific behavior that could surprise a user (injected prompt, built-in search, abuse block) is decided at spec.

**Negative.** Five-arm matches replace four-arm matches across roughly a dozen sites (session notes, entry-point census). The `base_url.is_some()` predicates on the OpenAI-shaped path have to become variant predicates, which touches temperature policy and the session-hint site. `ModelInfo` grows one field. The picker learns one optional tag.

**Risks.** Venice changes its `/models` payload field names: mitigated by the tag being optional and by the scope recording the verified path. Venice changes `venice_parameters` semantics: the flag is the only field sent, and it is documented by Venice. A future contributor treats Venice as "OpenRouter with a URL": D1's variant requirement and Reed's `base_url` census are the guard; the temperature and session-hint sites are named so they are checked. Privacy wording drift in copy: the copy constraint is in §28.5 and INVARIANTS.

## Alternatives considered

**Reuse `AiProvider::OpenRouter` with a base URL override.** Rejected; see D1. Two shipped behaviors treat `Some(base_url)` as OpenClaw.

**Store the Venice base URL in `ai_settings`.** Rejected: OpenRouter's is a constant, no user has asked to point Venice elsewhere, and a setting needs a key, writer, control, and migration disposition (ADR-0061 D7) for no benefit.

**Toggle default on.** Rejected; see D3. Native behavior is offered, not defaulted.

**Toggle stored per agent in `config_json`.** Rejected: the toggle is about which text reaches Venice's servers under the agent's documents, a connection-level fact like the key itself, not an agent trait. Per-install matches the card's other controls.

**Map Venice's tier strings to Pane vocabulary (e.g. "Private" to a lock icon with Pane copy).** Rejected: any Pane rendering beyond the verbatim string is a Pane privacy claim, which is locked out.

**Enable Venice's built-in web search instead of, or beside, Pane's web tools.** Rejected; see D2. It bypasses every mechanism §4.6 names.

**Withhold web tools from Venice at launch pending #2734.** Rejected by Matt 2026-08-29: the ADR-0064 gates are runtime-keyed and already proven on three runtimes; Venice models are tool-callers on the same wire format. #2734 is about per-model capability, which is orthogonal to the runtime decision.

**Surface deprecation headers in this PR.** Rejected; see D9.

**Separate supplement ADR for the allowlist change.** Rejected; see D10.

## References

- #2455, #2734, #2591 (web tools), #2522 (runtime-per-agent arc)
- ADR-0044 (keychain), ADR-0046 (text fallback), ADR-0061 D7 (dark-config rule), ADR-0063 D1/D4/D8, ADR-0064 D3/D4/D5/D7
- docs/spec/ai-integration.md §4.3.4 (new), §4.6.2, §4.6.10, §4A.2.4, §28.2, §28.3, §28.4, §28.5 (new); docs/spec/agents.md §24.2, §24.5.5; docs/spec/sessions.md §30.7.2; docs/INVARIANTS.md "Web Browsing Tools"
- docs/audits/2026-08-06-venice-ai-research.md §1, §3, §4, §5, §7
- docs/audits/2026-08-29-venice-spec-source-read.md @ 96958857
- docs/audits/2026-08-29-venice-spec-session-notes.md (in-session decisions, push-backs, entry-point census, evidence requirements)

---

## Supplement S1 (2026-09-01): Universal provider offering on selection surfaces

**Status:** Accepted. Supplements D8; supersedes its per-surface enumeration approach for listing surfaces.

D8 and prior ADRs enumerated which providers each selection surface offers (Smart Features internal provider, onboarding steps, pickers). This does not scale: every new runtime forced spec edits to listing text (Matt, 2026-09-01).

**Rule:** Any configured provider is offered on every provider-selection surface unless a stated capability gate excludes it. Capability gates are named in spec with a mechanism, not a provider list. Current gates: embeddings surfaces require a provider with an embeddings API (excludes Venice, OpenClaw); web browsing tools remain an explicit per-runtime opt-in under ADR-0064 D3, which this supplement does not touch; OpenClaw is excluded from key-provider-shaped surfaces by its gateway pairing model where the spec says so.

**Consequence:** Adding a runtime requires no spec edits to listing surfaces. The entry-point census for new runtimes replaces "update each listed surface" with "verify the universal rule reaches each surface." Smart Features offering Venice (#2795) is the first exercise of this rule, not a spec change.
