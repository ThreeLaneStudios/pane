# ADR-0009: OpenClaw Gateway Provider Semantics

*Line citations predate the #3097 module split.*

**Status:** Accepted  
**Date:** 2026-04-21  
**Author:** Reed (Architecture Audit)  
**Supersedes:** None

## Context

Between 2026-04-18 and 2026-04-21, we shipped 4 consecutive PRs attempting to fix OpenClaw gateway onboarding:
- #569: Model picker + AI greeting in onboarding
- #578: Add user message to AI greeting  
- #579: Model fetch via Tauri + greeting through chat pipeline (ATS fix)
- #580: Skip model picker for OpenClaw users + error-resilient greeting

Each PR fixed a surface symptom and exposed a deeper issue. The pattern revealed a systemic problem: **Pane's AI provider layer assumes OpenRouter semantics throughout.**

### The Fundamental Conflict

**OpenRouter model selection:**
- Model names are real: `anthropic/claude-opus-4`, `google/gemini-pro`
- Pane selects model, sends to OpenRouter
- OpenRouter routes to appropriate provider

**OpenClaw gateway routing:**
- "Model" IDs are routing targets: `openclaw`, `openclaw/main`, `openclaw/researcher`
- Gateway expects model field to specify WHICH agent to route to
- Gateway handles auth, provider selection, and actual model choice internally

### What We Discovered

Audit of the full provider path (see `docs/audits/2026-04-21-openclaw-provider-audit.md`) found 14 OpenRouter assumptions:

**Critical conflicts (BLOCKER severity):**
1. When `is_openclaw=true`, Pane hardcodes body model to `"openclaw/main"` AND adds `x-openclaw-model: <user-selection>` header
2. Gateway reads body model for routing → always routes to main agent
3. `x-openclaw-model` header conflicts with gateway's own routing → ignored
4. Result: User selection ignored, always get main agent

**Smart routing incompatibility (BLOCKER severity):**
1. Classifier calls need an "internal provider" (cheap LLM for yes/no)
2. Backend blocks OpenClaw as internal provider with error: "Gateway doesn't support x-openclaw-model for models it doesn't have keys for"
3. User with ONLY gateway configured can't use smart routing
4. Error message tells them to add OpenRouter (defeating purpose of gateway)

**Root cause:** The provider path in `openrouter.rs` was built for OpenRouter. Gateway support was bolted on via conditional logic (`if is_openclaw { ... }`). This creates conflicts because the two providers have incompatible semantics.

### What if we didn't decide?

We'd continue the pattern: ship a PR, discover new conflict, patch with more conditionals, repeat. Each patch makes the code more fragile. Eventually we'd have a maintenance nightmare where every provider change requires hunting through scattered `if provider == "openclaw"` checks.

The 4-PR cycle proves this approach doesn't work.

## Decision

**OpenClaw gateway gets its own provider semantics within the shared OpenAI-compatible streaming path.**

This is NOT a separate code path — both OpenRouter and OpenClaw use `openrouter.rs` for streaming (they're both OpenAI-compatible). The change is: where the code currently assumes OpenRouter behavior, it branches on the provider enum to apply the correct semantics for each. Shared logic stays shared. Provider-specific logic uses `matches!(provider, AiProvider::OpenClaw { .. })` instead of URL heuristics.

Specifically:

### 1. Model Field Semantics
- **OpenRouter:** `model` contains the actual model name. Pane chooses it.
- **Gateway:** `model` contains a routing target. User chooses agent, not model.

Implementation:
```rust
// BEFORE (openrouter.rs:172)
let body_model = if is_openclaw { "openclaw/main" } else { &request.model };

// AFTER
let body_model = &request.model;  // Use AS-IS for all providers
```

### 2. Header Usage
- **OpenRouter:** No special headers needed (or use HTTP-Referer/X-Title for attribution)
- **Gateway:** NO `x-openclaw-model` header (routing target IS the model field)

Implementation:
```rust
// REMOVE lines 199-202 and 403-406 entirely
// The x-openclaw-model header creation logic
```

### 3. Provider Detection
- **Current:** URL pattern matching (`base_url.contains("localhost")`)
- **New:** Provider type matching (`matches!(provider, AiProvider::OpenClaw { .. })`)

Implementation:
```rust
// BEFORE (openrouter.rs:167-169)
let is_openclaw = self.base_url.contains("localhost") || 
                  self.base_url.contains("127.0.0.1") || 
                  self.base_url.contains(":18789");

// AFTER
let is_openclaw = matches!(self.provider, AiProvider::OpenClaw { .. });
// or in streaming context where provider type unavailable:
let is_openclaw = openclaw_base_url.is_some();
```

### 4. Smart Routing Internal Provider
- **Current:** Gateway blocked from being internal provider
- **New:** Gateway CAN be internal provider (it routes classifier calls same as any other call)

Implementation:
```rust
// REMOVE lines 225-232 in commands.rs (the openclaw check)
// Gateway works fine for classifier — just pass routing target as model
```

### 5. UI/UX Adjustments
- When provider=openclaw, label dropdown "Select Agent" not "Select Model"
- Accept routing targets (`openclaw/main`) as valid "models" — no format validation
- Onboarding: either let user pick agent OR document that `openclaw/main` is default

## Consequences

### Positive
- **Fixes all 14 audit findings** with clean architectural separation
- **Unblocks smart routing** for gateway-only users (Tier 3)
- **Simpler code:** Remove conditional header logic, URL pattern matching, model name override
- **Future-proof:** Easy to add new providers without touching OpenRouter-specific code
- **Better UX:** Agent selection works as expected, no hidden overrides

### Negative
- **Breaking change for existing gateway users:** If anyone has workflows depending on `x-openclaw-model` header, they break
  - **Mitigation:** Gateway already ignores this header when routing target is in body, so no actual breakage
- **Need to audit cache markers:** Anthropic prompt caching syntax sent to gateway — works if gateway routes to Anthropic, ignored otherwise
  - **Mitigation:** Accept current behavior (harmless), document as Anthropic-specific
- **Shared path with cleaner branching:** `openrouter.rs` still handles both OpenRouter and Gateway, but with provider enum checks instead of URL heuristics. This is NOT a distinct code path — it's the same path with explicit branching where semantics differ.

### Risks
- **Incomplete provider abstraction:** No trait/adapter pattern for provider-specific behavior
  - **Impact:** Provider logic still scattered, harder to add new providers
  - **Mitigation:** Accept current state, refactor to trait-based polymorphism later (separate ADR)
- **Gateway capability discovery:** Pane doesn't know what gateway supports (tools, caching, etc.)
  - **Impact:** Can't optimize based on actual routing target's capabilities
  - **Mitigation:** Assume OpenAI-compatible API, accept feature parity limitations
- **Session persistence semantics differ:** Gateway uses `user` field for sessions, OpenRouter doesn't
  - **Impact:** Project switching creates new gateway sessions
  - **Mitigation:** Document as expected behavior OR use stable user ID (separate decision)

## Alternatives Considered

### Alternative 1: Keep Patching with Conditionals
**Approach:** Continue the #569-#580 pattern — fix each symptom as users discover it.

**Rejected because:**
- Already proven to fail (4 PRs, still broken)
- Code becomes unmaintainable spaghetti
- No end state — keeps getting more complex

### Alternative 2: Separate `gateway.rs` Module
**Approach:** Create new module for gateway streaming logic, separate from `openrouter.rs`.

**Rejected because:**
- 80% code duplication (both are OpenAI-compatible streaming)
- Two places to maintain cache logic, error handling, retry logic
- Doesn't solve provider abstraction problem, just moves it

**When this becomes right:** If we add 3+ OpenAI-compatible providers with different semantics, extract shared logic to `openai_streaming.rs` and use adapters.

### Alternative 3: Do Nothing, Deprecate Gateway Support
**Approach:** Focus on Tier 1/2 (direct API keys), drop Tier 3 (gateway).

**Rejected because:**
- The 2-machine OpenClaw setup is our primary use case — Matt uses it daily
- Gateway is a key differentiator (privacy-first, self-hosted)
- Not optional: both paths must work flawlessly

## References

- **Audit:** `docs/audits/2026-04-21-openclaw-provider-audit.md` (full findings with line numbers)
- **Related PRs:** #569, #578, #579, #580 (the 4-PR cycle that triggered this)
- **Provider enum:** `src-tauri/src/ai/provider.rs:40-48`
- **Streaming code:** `src-tauri/src/ai/openrouter.rs` (all provider streaming logic)
- **Model resolution:** `src-tauri/src/ai/commands.rs:resolve_model_for_message()`
- **Original x-openclaw-model PR:** #384 (where header logic was introduced)

## Implementation Plan

### Phase 1: Critical Path (Unblock Onboarding)
**Target:** v0.35.0 (next release)

1. Remove `x-openclaw-model` header logic (openrouter.rs:199-202, 403-406)
2. Remove hardcoded `openclaw/main` body model override (openrouter.rs:172, 375)
3. Fix provider detection to use enum instead of URL (openrouter.rs:167-169, 370-372)

**Test:** Gateway user can select `openclaw/researcher`, messages route to researcher (not main).

### Phase 2: Smart Routing (Feature Parity)
**Target:** v0.36.0

4. Remove gateway block in `get_internal_provider()` (commands.rs:225-232)
5. Remove Settings UI lock (Settings.svelte:2097-2111)

**Test:** Gateway user with no OpenRouter key can enable smart routing, classifier calls work.

### Phase 3: Polish (UX)
**Target:** v0.37.0

6. Change UI labels when provider=openclaw ("Select Agent" not "Select Model")
7. Improve onboarding agent selection OR document default
8. Make error parsing robust for gateway error formats

### Phase 4: Technical Debt (Low Priority)
**Target:** Backlog

9. Replace string provider comparisons with enum pattern matching
10. Remove OpenRouter URL fallback
11. Make HTTP headers conditional (OpenRouter-only)

## Success Metrics

- [ ] Gateway user can complete onboarding without errors
- [ ] Agent selection works (messages route to selected agent)
- [ ] Smart routing works with gateway as sole provider
- [ ] No new gateway-related issues filed for 2 weeks post-release
- [ ] Test suite has end-to-end gateway coverage

## Review Notes

**For reviewers:**
- Read audit doc first (`docs/audits/2026-04-21-openclaw-provider-audit.md`)
- Each fix addresses specific audit finding (cross-reference by finding ID: B1, H2, etc.)
- Breaking change mitigation: existing users won't break (header was already ignored)
- No spec changes needed (this is implementation detail)

**Open questions:**
1. Should we extract OpenAI-compatible streaming to shared module NOW or wait for 3rd provider?
2. Session persistence: stable user ID or accept project-based sessions?
3. Model fetch UX: show routing targets as "Agents" or keep as "Models"?

---

**Status update (if accepted):** Move to Accepted once PR implementing Phase 1 merges.
