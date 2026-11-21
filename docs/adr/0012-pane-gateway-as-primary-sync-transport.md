# ADR-0012: Pane Gateway as Primary Sync Transport for Split-Machine Tier 3

**Status:** Accepted
**Date:** 2026-04-27
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (supersedes §22B as Tier 3 split-machine transport on cutover; bridge plugin remains in production until §22C.16 criteria are met)

## Context

Split-machine Tier 3 — a Pane client on one machine talking to a self-hosted OpenClaw instance on another — is the deployment Matt runs personally and the one that has to work for self-hosted users at launch. Today, that traffic flows through `pane-bridge`, an OpenClaw plugin that exposes six HTTP endpoints (`/bridge/push`, `/bridge/pull`, `/bridge/pull/confirm`, `/bridge/sync-all`, `/bridge/models`, `/bridge/health`). The plugin grafts an HTTP surface onto OC, which is otherwise WebSocket-RPC-only at its core (147 WS methods, 0 core HTTP endpoints per Reed's 2026-04-27 OC endpoint inventory).

The bridge plugin is production-validated for sync but has accumulated architectural debt that the 2026-04-27 bridge audit (#689) documented across 11 gaps. Three are load-bearing for this decision:

- **Gap #6 — Zero test coverage.** No unit, integration, or end-to-end tests against the bridge endpoints. All validation has been Matt's manual dogfooding.
- **Gap #11 — Plaintext HTTP.** Bridge runs HTTP, not HTTPS. Mitigated today only because Matt's deployment uses Tailscale, which encrypts at the network layer. Any non-Tailscale deployment is plaintext on the wire.
- **Gap #7 — Architectural decisions undocumented.** The bridge-vs-Supabase split, HTTP-vs-WS choice, and ID-routing migration (#364) were shipped without ADRs. Future engineers reading the spec can't reconstruct intent from artifacts.

Reed's bridge-gateway scoping audit (2026-04-27) examined whether to address these by hardening the bridge in place or by replacing it with a Pane-owned gateway service. The data the audit produced was sound:

- ~70% of `sync_client.rs` is transport-agnostic — queue, conflict resolution, write integrity, agent routing — and ports cleanly to a different transport. Specifically: 21% strictly transport-agnostic (~240 lines), 21% HTTP-coupled (~240 lines), 58% mixed (~660 lines where trigger and merge logic are agnostic but the HTTP calls in the middle are not).
- The bridge's six endpoints absorb cleanly into a gateway service. None are intrinsic to OC; they are Pane infrastructure that happens to live as a plugin.
- Tier 1 (hosted, post-launch) cannot run a per-user OC plugin in a multi-tenant environment. A Pane-owned gateway service generalizes to multi-tenant hosting via a config flip rather than a refactor.

Reed's recommendation from that audit was to defer the gateway build and harden the bridge in place. The recommendation was calibrated to a 4–9-week build estimate based on standard contractor velocity. Matt's team builds significantly faster than that baseline, and the gateway scope (validated in the spec session, with §22C.11.3a outcome B locked as the active assumption per §22C.21) is 9.5–15.5 days plus a 7-day dogfood window — not 4–9 weeks. The data behind the recommendation was sound; the recommendation itself was velocity-mismatched and is not being adopted.

### What if we didn't decide

We continue running the bridge plugin in production, accept the gaps, and revisit when Tier 1 forces our hand. Concrete consequences:

- The three load-bearing gaps stay open. Plaintext HTTP keeps working as long as every user runs Tailscale; the day someone runs the bridge over an open LAN, that's a wire-readable secret leak. Zero test coverage means the next refactor of the bridge — or any Pane-side change to `sync_client.rs` — relies on Matt's manual verification, which is the failure mode mechanical-dev-hygiene (ADR-0008) was filed to end.
- Tier 1 forces a from-scratch gateway build under launch pressure with no dogfood runway. The work doesn't disappear; it gets harder.
- Same-machine keeps running redundant bridge logic, which means the bridge plugin keeps existing as production infrastructure. Every bridge bug is a Pane bug forever.

## Decision

The Pane Gateway becomes the primary and sole sync transport for Tier 3 once §22C.16 cutover criteria are met. The bridge plugin retires fully (per §22C.11.4 and ADR-0014's bridge-vs-Supabase framing).

The gateway is a Pane-owned service. It exposes HTTP/2 + SSE to the Pane client, holds tenant-scoped state, terminates TLS, and speaks WebSocket to OC for the eleven OC RPC methods Tier 3 requires (sync absorption + ten chat/session/models methods). The full spec is `docs/spec/sync.md` §22C.

The gateway is single-tenant at launch with the multi-tenancy seam present (every state row carries `tenant_id`, every endpoint handler reads `tenant_id` from auth context). Tier 1 hosted is a config flip, not a refactor.

The decision to proceed against Reed's defer-recommendation is recorded in this ADR explicitly. Reed's data (the ~70% transport-agnostic finding, the bridge gaps, the Tier 1 forward-compatibility argument) is foundational and adopted in full. Reed's defer-conclusion was calibrated to a 4–9-week contractor-velocity build estimate; the spec session validated the actual scope at 9.5–15.5 days (post §22C.11.3a outcome B), which makes the build fit into the current sprint without delaying launch.

## Consequences

### Positive

- **Tier 1 forward-compatibility achieved without future refactor.** The multi-tenancy seam (tenant-scoped state, per-tenant OC credentials table, per-tenant rate-limit hooks) is in place from day one. Hosted launch is a config change.
- **Bridge audit gaps #6, #9, #10, #11 close as the gateway lands.** Tests are blocking for cutover (§22C.15, seven categories). TLS is architectural, not Tailscale-dependent. Error classification covers 401/403/429/5xx with explicit retry semantics. Payload caps are hard-rejected at 10 MiB push / 1 MiB session message.
- **Pane client stays clean HTTP/2.** No WS client in Pane. The gateway absorbs OC's protocol asymmetry (WS-only core) so the Pane↔gateway interface is uniform request/response + SSE for streaming.
- **Bridge plugin retires fully.** No two-system maintenance burden post-cutover.
- **Velocity argument validated.** The spec session ran end-to-end in a single sitting with concrete sizing, which is the evidence that Reed's contractor-velocity assumption doesn't apply here.

### Negative

- **One more service to operate on the OC machine.** Users running split-machine deploy the gateway alongside OC. Onboarding adds a pairing step (URL + 6-digit code + cert fingerprint).
- **Pre-launch threat model is constrained to Tailscale-or-LAN.** Multi-tenancy isolation is structural (tenant-scoped queries) but not enforced against attackers. Hard requirement before Tier 1 hosted launches: real tenant resolution from token, multi-tenant pairing UI, OC connection pooling.
- **Self-signed TLS trades familiar UX for architectural simplicity at launch.** Users see a fingerprint string to verify during pairing rather than a green padlock. The trade is deliberate: Let's Encrypt requires DNS-01 ACME for non-public hostnames and adds rotation infrastructure that doesn't earn its keep at single-tenant launch. Fingerprint pinning during pairing is robust against MITM. §22C.20 spike #2 revisits whether ACME support belongs at launch or post-launch — this is a calibration question, not a gap.
- **Cutover window of dual-stack operation.** Bridge plugin keeps running during gateway build (per §22C.16). Two transports live at once until Matt signs off on cutover.

### Risks


- **OC's `sessions.messages.subscribe` buffering semantics are unspecified.** §22C.20 spike #1 measures actual gap rate. Mitigation: the gateway maintains its own per-session ring buffer (1000 events / 5 min), so client-visible reliability does not depend on OC's behavior — gaps surface as `replay.gap` events with paginated refetch via `GET /v1/sessions/:id/messages?since=...`.
- **Velocity assumption could prove optimistic.** If the build runs longer than 13.5 days, launch timing shifts. Mitigation: scope is explicitly minimal (§22C.7.1 whitelist of eleven OC methods, deferred ~23 Tier 3-tagged methods) and the test suite is the only category with real variance. Cutting test categories cuts the rock-solid promise; we don't.
- **Bridge plugin coexistence during cutover window.** Gateway and bridge can both run on the same OC machine without conflict, but verifying that requires explicit testing (covered by §22C.16 cutover criterion #4 — "Pane on gateway exclusively for ≥7 days," with bridge unloaded).

## Alternatives Considered

**Defer gateway, harden bridge in place (Reed's recommendation).** Strongest alternative. Closes audit gaps #6 (write tests), #9 (error classification), #10 (payload limits), and #11 (TLS) inside the bridge. Estimated 1–2 weeks. Rejected for two compounding reasons: (1) Tier 1 forces a from-scratch gateway anyway and the work doesn't transfer cleanly because the bridge plugin model can't run multi-tenant; (2) the velocity assumption underlying Reed's 4–9-week alternative-cost estimate doesn't match this team. Doing both — harden bridge then build gateway later — costs more than building the gateway once.

**Bridge plugin with TLS termination by Tailscale only.** Document the Tailscale dependency as a deployment requirement; ship as-is. Rejected: makes Tailscale a hard product dependency, fails the hosted Tier 1 forward-test entirely, doesn't address gaps #6/#7/#9/#10. The only gap it touches is #11, and only by externalizing.

**Direct WS from Pane to OC (skip the gateway entirely on Tier 3).** Pane grows a WS client and speaks OC's RPC protocol natively. Rejected: bakes OC's protocol weirdness into Pane permanently, no clean Tier 1 path, every OC protocol change becomes a Pane release. The "absorb OC's protocol asymmetry behind a clean HTTP/2 + SSE interface" framing is the load-bearing architectural argument and ADR-0015 captures it independently.

**Do nothing.** Continue running the bridge with its current gaps, address Tier 1 when forced. Rejected per "What if we didn't decide" above.

## References

- Spec section: `docs/spec/sync.md` §22C (full gateway spec); §22B marked LEGACY on cutover
- Audits (all 2026-04-27, in `~/Shared/ExampleApp/docs/audits/`): bridge-audit (#689), bridge-gateway-scoping, oc-endpoint-inventory, same-machine-tier3-inventory
- Related ADRs: ADR-0002 (CRDTs for user-data sync — coexists with gateway path); ADR-0013 (auth and multi-tenancy seam); ADR-0014 (bridge vs Supabase split, formalizes the two-system decision); ADR-0015 (gateway↔OC transport)
- INVARIANTS to file alongside this ADR: §22C.18 lists 14 — the gateway-introduced subset (gateway-as-sole-transport, Tier 2 never routes through gateway, gateway terminates TLS, cross-tenant access impossible by construction, opaque bearer tokens at launch, oc_agent_id as routing key, queue invariants port forward, atomic write + read-back checksum, WS subscription gaps invisible, no chat.send/chat.abort, gateway invokes only the §22C.7.1 whitelist, payload caps are hard rejects, reconciliation logic lives in Pane)
- Velocity baseline: Dev Hygiene Playbook §1 (spec discipline), §11 (system audits) — the framework that makes 9.5–15.5 days credible at this team's velocity
