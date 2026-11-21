# ADR-0055: OC Gateway Handshake-Timeout Recovery Surfacing

**Status:** Proposed
**Date:** 2026-07-13
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0013 (Auth and Multi-Tenancy Seam — unchanged; this ADR adds a recovery-surfacing contract on top of its gateway→OC credential model)
**Related:** ADR-0015 (gateway↔OC transport — the path whose handshake times out); §22C.5.4 (recovery contract); §22C.5.3 (gateway→OC auth); §22C.6.5 (OC WS connect flow)

## Context

ADR-0013 established that OC owns the gateway→OC credential and the Pane gateway holds one OC bearer token at launch (`tenant_oc_credentials`, single row; surfaced operationally as the `OC_BEARER_TOKEN` environment variable read once at startup by `src-gateway/src/main.rs`). There is no programmatic path for the gateway to obtain or refresh that token: re-pairing refreshes only the Pane↔gateway token (`pair.rs`), never `OC_BEARER_TOKEN`. This two-copy split is the intentional Tier-1 tenancy seam and is not in question here.

The gap this ADR closes is recovery UX when OC rotates its gateway token out from under the gateway. Live split-machine testing on 2026-07-12 hit this: after the 2026-07-07 `openclaw update` / SecretRefs migration rotated OC's token, the gateway kept presenting the old one. Web research confirms this rotation-vs-service-file drift is recurring OC behavior (updates, `openclaw doctor --fix`, config reset, and setups that auto-rotate), so it will reach future two-machine OC users generally.

The failure is silent by construction. OC validates the token at the connect-*frame* layer, after the WebSocket upgrade (§22C.6.5), so a stale token opens the socket, then produces no rejection frame and no `hello-ok`. The gateway's 10 s hello-ok wait (`oc_client.rs`) elapses and — before this ADR — collapses into an opaque 502 on `/v1/agents` and `/v1/sync/initial`, indistinguishable to Pane from any other OC-hop 502. The operator's only recovery is to update the service file and restart the gateway, which Pane gave no signal was needed.

A verbatim failure-arm log from the 2026-07-12 run did not survive (gateway stderr truncates on restart). The catch point is instead locked from source: among all 502-producing paths in the OC proxy, only the hello-ok-timeout arm carries a hard ~10 s bound (`connect_async` failures fail fast → `OcConnectionLost`; the auth-retry loop bounds at 5 s; the request timeout is 30 s → 504). The surviving tower-layer log line (`"response failed" … "latency":"10006 ms"`, `/v1/sync/initial`) is the fixed-timer fingerprint of that arm.

## Decision

Surface the gateway→OC handshake-timeout distinctly, and render an explicit, agent-actionable token-recovery message in Pane. Specifically:

1. **Dedicated error variant.** Introduce `GatewayError::OcHandshakeTimeout`, thrown at the hello-ok-timeout arm in `oc_client.rs`, replacing the generic `OcClient("Authentication timeout")` at that call site only. It is distinct from `OcConnectionLost` and from the 5 s auth-retry-loop string of the same wording. Detection keys on the variant, never on a string match.
2. **Wire code, status unchanged.** `OcHandshakeTimeout` maps to HTTP 502 carrying `error: "oc_handshake_timeout"` in the structured `ErrorResponse` body. Pane discriminates on the `error` code, matching how existing OC error codes already surface distinctly at 502.
3. **Recovery message.** Pane maps `oc_handshake_timeout` to a message that names a stale `OC_BEARER_TOKEN` as the likely cause (never asserted as certain), acknowledges OC-restarting/busy as the alternative, and gives the concrete fix: copy `~/.openclaw/openclaw.json` → `gateway.auth.token` into `OC_BEARER_TOKEN` in the Pane gateway service file (macOS launchd / Linux systemd), then restart the Pane gateway.

This is detection-and-surface only. Pane does not capture, refresh, or re-derive the OC token, and does not rewire re-pairing to touch it.

## Consequences

### Positive

- Future two-machine OC users get an actionable recovery path across a token rotation instead of an opaque 502. The message is executable from its own text by a user or their agent.
- The distinction lives in a typed variant, not a fragile string match — durable against the second, unrelated `"Authentication timeout"` producer (the 5 s auth-retry loop).
- The two-copy token architecture (ADR-0013) is untouched; this is purely additive surfacing.

### Negative / accepted limits

- **Cannot auto-remediate.** OC owns the token and the gateway reads it startup-only by design. The ceiling is telling the operator exactly what to do; the fix remains manual (edit service file + restart). Automating it would require capturing or refreshing OC's token, which ADR-0013 and this ADR's scope explicitly reject.
- **Signal is a superset, not a certainty.** The hello-ok-timeout bucket is "OC reachable, handshake didn't complete" — dominated by stale-token but also reachable by OC wedged/restarting immediately post-upgrade. The message is therefore likely-cause framed. Accepted: the narrower, honest signal beats either a false-certain token claim or a generic "can't reach OC" that discards the distinction the source already makes.
- **Deployment-path coupling in copy.** The recovery message names service-file locations and a restart command, which vary by OS and install. Mitigated by giving both macOS and Linux forms and pointing at the config source of truth rather than hardcoding one operator's layout.

## References

- Spec: `docs/spec/sync.md` §22C.5.4 (recovery contract, new), §22C.5.3 (gateway→OC auth), §22C.6.5 (OC WS connect/hello-ok flow)
- ADR-0013 (auth & multi-tenancy seam — supplemented, unchanged), ADR-0015 (gateway↔OC transport)
- Source: `src-gateway/src/oc_client.rs` (hello-ok-timeout arm; `connect`/`ensure_connected_and_send`), `src-gateway/src/error.rs` (`GatewayError`/`ErrorResponse` mapping), `src-gateway/src/main.rs` (`OC_BEARER_TOKEN` startup read)
- Origin: 2026-07-12 split-machine diagnostic; catch-point locked from source (Case-A latency analysis, 2026-07-13)
