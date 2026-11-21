# ADR-0067: Explicit Agent Selection on the OC Wire

**Status:** Accepted
**Date:** 2026-09-21
**Author:** Matt Carano
**Supersedes:** None
**Related:** #2900, ADR-0023, ADR-0062, ADR-0063

Status: Accepted 2026-09-21. Relates to #2900. Cites ADR-0023, ADR-0062, ADR-0063.

## Context

OpenClaw 2026.8.2 rejects any chat request that does not name its agent when more than one agent is configured. Pane relied on OC's implicit default, and pane-gateway rewrote the model field to bare `openclaw` before forwarding, discarding whatever the app sent. Every Pane session is bound to one agent (ADR-0062), but the binding never crossed the wire. The app also classified OC's 400 as a connection failure and retried it five times.

Two things spec did not express caused this. The identity a session binds to (`ai_agents.id`, Pane-local) and the identity OC knows (`ai_agents.runtime_ref`) were never distinguished. And default-agent resolution existed as four hand-written copies that had diverged.

## Decisions

1. **Model-prefix selection.** Every chat or stream request to OC names its agent by setting `model` to `openclaw/<agentId>`. The `x-openclaw-agent-id` header is not used: one selection mechanism, and the model field is already wired end to end across both binaries. A header would be a second path to keep in sync.

2. **Only `runtime_ref` crosses the wire.** `agentId` is the bound agent's `ai_agents.runtime_ref`. `ai_agents.id` is Pane-local and never leaves the app. When `runtime_ref` is NULL (local and AI-created agents), the request goes out as bare `openclaw`, never as the local id.

3. **Gateway rewrites only bare requests.** pane-gateway passes an agent-scoped model (`openclaw/…`, `openclaw:…`, `agent:…`) through unchanged and stamps `openclaw` only when the client sent a bare, absent, or catalog model id. This keeps an older app build working against a newer gateway. The parameter-compat strip table resolves against the selected agent, not a hardcoded `main`.

4. **A gateway 4xx is a provider error.** It is never retried, never enters the reconnect loop, and always surfaces OC's own message text. 401, 402, and 403 keep their specific codes ahead of the general 4xx branch. 5xx and transport failures keep today's reconnect behavior.

5. **One default-resolution order, stated once.** (1) The default OC reports in `agents.list` (per-agent `isDefault`). (2) The user's Settings choice (`ai_agents.is_default`, set through `DefaultAgentPickerModal`). (3) The agent whose `runtime_ref` is `main`. This replaces the four independent implementations. Against OC 2026.8.2 every agent reports `isDefault: false`, so step 1 does not fire; step 2 is the production path and step 3 is the first-run path.

6. **Deployment order.** pane-gateway ships first and is backward compatible with the installed app. The app build follows. Between the two, the 400 is not fixed; the gateway change alone changes nothing for an old app. A non-regression test gates the gateway-first step.

## Consequences

Renaming or removing OC's default agent cannot take Pane offline. The `agents.list` shape and the `openclaw/<agentId>` routing contract are captured in spec under ADR-0023 with version, date, and method; the stale fixture in `src-gateway/src/agents.rs` that asserted a `defaultId` OC does not send is replaced by a reference to the spec capture. A green test for step 1 of default resolution is not evidence the feature works on the current gateway; the PR description says so.

**Ruling (Matt, 2026-11-11, #3106).** The gateway forwards to whichever agent the authenticated client names; binding is enforced in Pane.
