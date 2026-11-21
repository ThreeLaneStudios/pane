# ADR-0046: Native-first tool-call parsing with a provider-agnostic text fallback

*Line citations predate the #3097 module split.*

**Date:** 2026-06-22
**Status:** Proposed
**Supersedes:** §4.3 "skip text parsing entirely" stance (for the agentic loop only)
**Relates to:** #1586, §4.5.8

## Context

§4.3 specified native tool calling only and explicitly rejected text parsing as "more work AND
less reliable — skip it entirely." A text-fallback was nonetheless built, gated `is_openclaw`,
because real models (early OC prototype) emitted tool calls as text rather than via the
structured field. The gate was an artifact of *when* it was built, not a decision that only OC
needs it.

#1586 triage (2026-06-22) found the same weakness affects OR and Ollama — shipped at launch,
capable of running non-native-tool-calling models — where text-emitted calls are silently
dropped. Separately, Ollama parses no tool calls at all (native included): `ollama.rs:374`
returns `Ok((full_text, vec![]))` unconditionally with the comment "Ollama doesn't support
tool calls yet."

Source confirmation (2026-06-22, main `39911015`):
- `parse_text_tool_calls` at `commands.rs:5032` accepts `{"tool": "<name>", "args": {...}}`,
  fenced or bare
- Text-bridge gated `is_openclaw && tool_calls.is_empty()` at `commands.rs:6691`
- Ollama stream parser (`ollama.rs:339-368`) reads `message.content` only, never
  `message.tool_calls`

## Decision

Native parsing is the contract for every provider and runs first. A text-fallback is a
sanctioned resilience layer, provider-agnostic, that runs **only** when native parsing returns
zero calls (empty-result guarded, single-fire). This explicitly overturns §4.3's "skip it
entirely" for the agentic loop: text parsing is retained, but as a guarded fallback behind
native, not a primary path.

## Consequences

### Positive

- Non-native-tool-calling models on OR/Ollama become usable in the agentic loop instead of
  silently failing.
- The invariant (native-first, single-fire, provider-agnostic) is mechanically enforceable —
  one guard (`tool_calls.is_empty()`), no provider gate.

### Negative

- False-positive text parsing is bounded by (a) the empty-result guard — the fallback never
  runs when native produced a call — and (b) a narrow recognized-pattern set. But it is not
  zero-risk: a model that writes `{"tool": "...", "args": {...}}` as illustrative prose (not
  intending to invoke a tool) could trigger a false positive. Accepted: the alternative
  (silently dropping real tool calls) is worse.

### Follow-ups

- §4.3's blanket "skip text parsing" sentence should be annotated to point at this ADR and
  §4.5.8 so the next reader doesn't re-derive the rejected stance. (Separate spec edit.)
- `INVARIANTS.md` one-liner mirroring the native-first / single-fire / provider-agnostic
  invariant, alongside the code PRs.
- The text-bridge generates `call_id: "openclaw_text_{counter}"` — once de-gated, rename the
  prefix to something provider-neutral (implementation detail for the code PR).
