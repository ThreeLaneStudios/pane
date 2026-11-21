# ADR-0015: Gateway↔OC Transport — WS Where Required, HTTP Where Supported

**Status:** Accepted
**Date:** 2026-04-27
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The Pane Gateway (ADR-0012) sits between the Pane client and OpenClaw. The Pane↔gateway protocol is locked: HTTP/2 for request/response, SSE for streaming session messages. The remaining transport question is gateway↔OC. Three choices exist, and the wrong one bakes OC's protocol shape into Pane permanently.

Reed's 2026-04-27 OC endpoint inventory established the constraints:

- OpenClaw's gateway core exposes 147 WebSocket-RPC methods and 0 native HTTP REST endpoints. Sessions, agents, models, chat, identity — everything is WS.
- The only HTTP surface on OC today is plugin-defined routes. The `pane-bridge` plugin contributes six (`/bridge/push`, `/bridge/pull`, `/bridge/pull/confirm`, `/bridge/sync-all`, `/bridge/models`, `/bridge/health`). None of these are core OC features; they're Pane infrastructure that happens to live as an OC plugin.
- The §22C launch surface uses eleven OC methods: `models.list`, `gateway.identity.get`, `agents.list`, `sessions.create`, `sessions.list`, `sessions.get`, `sessions.delete`, `sessions.send`, `sessions.messages.subscribe`, `sessions.abort`, `chat.history`. All eleven are WS-only on OC's core. The bridge plugin's HTTP routes are scheduled to retire post-cutover (ADR-0012, §22C.11.4).

The architectural question: should the gateway speak WS to OC for the methods that require it and HTTP to the bridge plugin for sync (mixed transport), or should the gateway speak WS to OC end-to-end (uniform transport)?

A subsidiary question: should the bridge plugin be hardened and kept as the HTTP surface for sync, with the gateway only handling the new WS-RPC methods? Or should the gateway absorb sync as well, replacing the bridge plugin entirely?

These decisions interact. The §22C launch design absorbs both: gateway speaks WS to OC for everything OC requires, gateway absorbs the bridge's sync responsibilities into its own surface (no HTTP calls to the bridge plugin), and the bridge plugin retires fully post-cutover.

### What if we didn't decide

Three predictable failure modes:

- **Mixed transport: gateway speaks WS to OC for chat/sessions, HTTP to the bridge plugin for sync.** The gateway becomes a fan-out across two protocols and two endpoints on the OC machine. The bridge plugin's gaps (audit #689 gaps #6/#9/#10/#11) stay open because the bridge plugin doesn't retire. Two protocols, two failure modes, two test matrices. The gateway also has to deal with the bridge plugin being a different process with independent lifecycle from OC core — restart races, version skew, port conflicts.
- **Pane talks WS to OC directly (no gateway absorption).** Pane grows a WS client and speaks OC's RPC protocol natively for chat/sessions. The bridge plugin keeps doing sync. This is "ship two transports in Pane and keep the bridge plugin." Rejected for the same reasons as the original gateway decision (ADR-0012) plus the additional cost of WS-client maturity in Pane (reconnect, replay, backoff, heartbeat all become Pane-side concerns instead of gateway-side concerns).
- **HTTP shim plugin on OC.** Build an OC plugin that exposes HTTP wrappers around OC's WS methods. Gateway speaks HTTP to that plugin. This trades the bridge plugin's six routes for ~50 plugin routes (one per Tier 3-relevant WS method), reintroducing the original problem at higher scale: an HTTP surface grafted onto an OC that doesn't natively expose one. We'd be writing the inverse of the gateway, on the wrong machine.

Each failure mode shares a common thread: avoiding the WS-to-OC reality means re-creating the bridge plugin's role under a different name, with the same gaps. Reed's OC inventory data is the load-bearing point — there is no native HTTP path on OC that the gateway can use. Pretending otherwise produces shadow infrastructure.

## Decision

The gateway uses **a single WebSocket connection to OC per tenant** as its only transport to OC. All eleven launch-surface OC methods are invoked via this connection. There are no HTTP calls from the gateway to OC, to the bridge plugin, or to any other OC-side service.

The Pane↔gateway interface remains uniform HTTP/2 + SSE. The gateway absorbs OC's WS-RPC asymmetry behind a clean request/response surface to Pane. Pane never grows a WS client.

The bridge plugin's role is fully absorbed by the gateway. Sync operations (`/bridge/push`, `/bridge/pull`, `/bridge/pull/confirm`, `/bridge/sync-all`) become gateway endpoints (`/v1/sync/push`, `/v1/sync/pull`, `/v1/sync/pull/confirm`, `/v1/sync/initial`) that operate against the gateway's tenant-scoped state and write to OC's filesystem on the OC machine via the gateway's local filesystem access. Model discovery (`/bridge/models`) becomes `/v1/models` backed by a WS call to `models.list`. Health (`/bridge/health`) becomes `/v1/health`.

Sync writes do not require a WS round-trip to OC. The gateway runs on the OC machine and writes directly to the agent identity files in OC's workspace via filesystem operations. OC's filesystem watcher (or restart-time integrity check) picks up the writes the same way it picks up bridge plugin writes today. This preserves the same-machine optimization the bridge plugin uses (writes are filesystem-local, not wire-transmitted) and doesn't require OC to expose a sync-write WS method.

The WS connection lifecycle, reconnect strategy, heartbeat, and replay buffer are spec'd in §22C.4.2 and §22C.7.3. Headlines: persistent connection, lazy-on-first-request open, 30s ping / 10s pong, exponential backoff reconnect (1s → 30s capped), per-session ring buffer (1000 events / 5 min) for subscription gap-invisibility (INVARIANT #10).

## Consequences

### Positive

- **Single transport to OC.** One protocol, one connection, one set of failure modes to test and reason about. Reconnect logic lives in one place.
- **Pane stays clean HTTP/2 + SSE.** No WS client in Pane, no protocol asymmetry leaking into the client. Pane interface remains stable across OC version changes.
- **Bridge plugin retires fully.** No two-system maintenance burden post-cutover. Bridge audit gaps #6/#9/#10/#11 close as the gateway lands. Same-machine moves to filesystem-direct (§22C.11), eliminating the bridge's redundant queue work.
- **Gateway is the ONE place that knows OC's protocol.** Any future OC RPC method addition is a gateway change, not a Pane change. Any OC protocol evolution (e.g., if OC ever adds native HTTP) is a gateway-internal optimization, not a Pane-facing concern.
- **Tier 1 forward-compatibility maintained.** Multi-tenant Tier 1 holds N WS connections (or pooled), one per tenant. The transport choice doesn't constrain the tenancy model.
- **Filesystem writes for sync stay local on the OC machine.** No WS round-trip per sync write. Latency profile matches the bridge plugin's today; performance does not regress on cutover.

### Negative

- **WS connection management is not free.** Heartbeat, reconnect, replay buffer, backoff — all real code that has to be tested under failure. §22C.15 test categories #1 (network failure) and #3 (reconnect-with-gap) are the heaviest part of the gateway test suite specifically because of this.
- **Single connection per tenant is a coordination point.** All eleven methods share a connection; if it fails, everything pauses until reconnect. Mitigation: replay buffer ensures gap-invisibility from Pane's perspective; gateway-side queue ensures sync writes aren't lost during reconnect.
- **Gateway must run on the OC machine (or have filesystem access to OC's workspace).** Sync writes are filesystem-direct; the gateway must be able to write to `.openclaw/workspace/{agent_uuid}/`. Same machine constraint as the bridge plugin today. If a future deployment shape separates gateway from OC, sync writes have to move back through WS — a §22C.20 spike candidate if/when that happens.
- **Eleven WS methods is the floor, not the ceiling.** As the launch surface grows toward the deferred ~23 Tier 3-tagged methods (sessions.compact, sessions.usage.\*, etc.), the WS surface widens. Mitigation: the launch whitelist is in §22C.7.1 and INVARIANT #12 ("gateway invokes only the §22C.7.1 whitelist"). Adding methods is intentional and scoped to follow-up specs.

### Risks

- **OC's `sessions.messages.subscribe` buffering semantics are unspecified.** If OC drops events during the gateway-side disconnect window (rather than buffering them), the gateway's replay buffer is the entire safety net. Mitigation: §22C.20 spike #1 measures this; the gateway design is correct regardless (replay.gap + paginated refetch covers either case).
- **WS protocol drift between OC versions.** OC could change RPC method shapes or rename methods between versions. Mitigation: the gateway is version-pinned to a known OC version at launch; OC version compatibility is part of the cutover criterion (§22C.16) and re-validated on OC upgrades.
- **Filesystem write contention if OC and gateway both write the same file simultaneously.** Two distinct failure modes need two distinct mitigations. Atomic write + read-back checksum (§22C.8.3, INVARIANT #9) prevents *torn writes* — corruption from a half-written file landing on disk. The conflict-resolution module (§22C.11.2a, INVARIANT #14) prevents *lost updates* — gateway clobbering OC's recent write because gateway didn't see it. The first ensures whatever lands on disk is well-formed; the second ensures the version that lands is the right one (timestamp comparison + LWW with gateway tie-break + content-identity skip, before the write). The same race exists today; the bridge plugin's queue handled it via the same conflict-resolution logic that the gateway now reuses through the shared module.
- **Single-WS-per-tenant could become a throughput bottleneck under heavy load (Tier 1).** Mitigation: connection pooling per tenant is a pre-Tier-1 hardening item and doesn't require protocol changes — pooling is a gateway-internal optimization invisible to Pane.
- **Reconnect storms if the OC instance restarts under load.** Mitigation: exponential backoff with cap (1s → 30s) prevents tight reconnect loops. Permanent failure (auth rejected) stops automatic reconnect and surfaces in `/v1/health`.

## Alternatives Considered

**Mixed transport: WS to OC for chat/sessions, HTTP to bridge plugin for sync.** The minimal-change alternative — keeps the bridge plugin in production, extends the gateway only with the new WS-backed surface. Rejected: bridge plugin gaps stay open (the explicit reason ADR-0012 exists), two transports complicate the gateway internally, and the bridge plugin's process lifecycle is independent from OC core which adds restart-race and version-skew failure modes. The "smaller scope" framing of this alternative is misleading because retiring the bridge plugin happens eventually anyway — doing it during the gateway build is cheaper than doing it later under launch pressure.

**Pane talks WS to OC directly (gateway only handles sync).** Move chat/sessions WS to Pane; gateway is sync-only. Rejected: Pane grows a WS client with its own reconnect/replay/heartbeat logic, OC's protocol becomes Pane's problem permanently, and the Tier 1 forward-compatibility argument from ADR-0012 fails (hosted Tier 1 cannot have Pane talking WS to N different user-owned OC instances directly through the hosted layer). Plus Pane stays cleaner with HTTP/2 + SSE — adding WS would mean the client maintains both transports forever.

**HTTP shim plugin on OC (replace bridge plugin with an HTTP↔WS bridge).** Build an OC plugin that wraps WS methods in HTTP. Gateway speaks HTTP end-to-end. Rejected for two reasons: (1) it reproduces the original bridge plugin problem at larger scale — an HTTP surface grafted onto OC, with all the same gap-classes (testing, error handling, payload limits, plaintext if not careful); (2) it puts the protocol-translation layer on the OC machine, where Pane has less control and where every OC version change is a plugin update. The gateway exists specifically to put protocol translation on the Pane side. Building an HTTP shim on OC inverts the architecture for no architectural gain.

**Native HTTP added to OC.** Lobby for OC core to expose HTTP-RPC equivalents of the WS methods. Out of scope for this ADR — even if accepted upstream, the timeline doesn't fit launch, and the gateway is needed regardless for Tier 1 forward-compatibility. If OC later adds native HTTP, the gateway can switch internally without affecting Pane.

**Per-method connection (not per-tenant).** Open a fresh WS for each RPC call. Rejected: connection setup overhead for every call, no persistent subscription path (`sessions.messages.subscribe` requires a long-lived connection), and OC's auth happens at handshake — every call would re-auth. Persistent connection per tenant is the standard WS-RPC shape for good reasons.

**Filesystem-direct sync without WS coordination.** Skip OC awareness of sync entirely; gateway writes files to OC's workspace, OC's filesystem watcher picks them up, no WS message needed. **This is what the decision actually does for sync writes.** Listed here for clarity: sync writes are filesystem-direct on the OC machine; WS is used for chat/sessions/models/identity/agents, not for sync. The "single transport to OC" framing in the Decision section is precise — WS is the only wire transport; filesystem is the local-machine sync mechanism.

## References

- Spec section: `docs/spec/sync.md` §22C.4.2 (gateway↔OC connection lifecycle); §22C.7.1 (method whitelist — eleven methods); §22C.7.3 (subscription replay & gap-invisibility); §22C.4.1 (Pane↔gateway transport — for contrast)
- Audits: 2026-04-27 OC endpoint inventory (the load-bearing data point: 147 WS methods, 0 core HTTP); 2026-04-27 bridge audit (#689) gap #8 (HTTP-vs-WS undocumented — this ADR closes it)
- Related ADRs: ADR-0012 (Pane Gateway as primary sync transport — establishes the gateway service this ADR's transport choice lives inside); ADR-0013 (auth and multi-tenancy seam — gateway↔OC auth handshake uses the per-tenant OC credential model defined there); ADR-0014 (bridge vs Supabase split — clarifies that this ADR's "single WS to OC" applies only to System 2; System 1 uses Supabase CRDT independently)
- INVARIANTS to file alongside this ADR (subset of §22C.18): #10 (WS subscription gaps invisible to Pane client), #11 (no chat.send/chat.abort — sessions.* canonical except chat.history which has no sessions parallel), #12 (gateway invokes only the §22C.7.1 whitelist). Cross-references INVARIANT #14 (Pane-side reconciliation) for the lost-update mitigation in Risks.
- Spike candidates: §22C.20 #1 (OC `sessions.messages.subscribe` buffering semantics — refines but does not change this ADR)
