# Onboarding Spec Rewrite + ADRs 24–26 — Session Notes

**Session date:** 2026-05-07
**Driver:** Claude (spec session) for Matt
**Branch target:** `docs/onboarding-redesign-959` (PR #960). Markdown for Brit to land; Matt merges.

---

## Status

| Deliverable | Status |
|---|---|
| ADR-0024 (T1, local-only decryption) | ✅ Drafted — `0024-local-only-decryption.md` |
| ADR-0025 (T2, ecom check cadence) | ✅ Drafted — `0025-ecom-check-cadence.md` |
| ADR-0026 (T3, install UUID for `active_machine_id`) | ✅ Drafted — `0026-install-uuid-active-machine-id.md` (incl. nit fix on sneak-around-via-reinstall framing) |
| External-contract captures (consolidated) | ✅ Drafted — `01-external-contract-captures.md` |
| §6 rewrite (`onboarding.md`) | ✅ Drafted — `02-spec-onboarding-section-6.md` |
| §22C.22 new sub-section (`sync.md`) | ✅ Drafted — `03-spec-sync-section-22C-22-onboarding-integration.md` |
| §22C.11 update (`sync.md`) | ✅ Drafted — `04-spec-sync-section-22C-11-update.md` |
| §22B.2 / §22B.3 / §22B.8A retirement banners (`sync.md`) | ✅ Drafted — `05-spec-sync-22B-retirement-banners.md` |
| §24.5 update (`agents.md`) | ✅ Drafted — `06-spec-agents-section-24-5-update.md` |
| README.md §0.3.2 update | ✅ Drafted — `07-spec-readme-section-0-3-2-update.md` |

All deliverables for the session are landed.

---

## Source-reads done inline (closing Matt's flagged gaps)

**Gap 1 — `/pair` error wire shape.** Reed had the `ErrorResponse` struct + variant→HTTP mapping but no live JSON. Closed by lifting the `IntoResponse` impl from `src-gateway/src/error.rs` and the integration tests in `src-gateway/tests/error_classification_test.rs` that pin the wire shape per variant. The `error` field values are snake_case, asserted by test (e.g., `assert_eq!(body["error"], "unauthorized")`). For `/pair` specifically, only `Unauthorized(_)` fires from the handler, with three message strings: `"Pairing code expired"`, `"Invalid pairing code"`, `"No active pairing code"`. Goes in the consolidated captures next turn.

**Gap 2 — `exchange_code_for_token` semantics.** Source-read from `src-gateway/src/auth.rs`:
- **Code TTL:** 5 min (`Duration::minutes(5)` in `generate_pairing_code`).
- **One-time use:** confirmed. The active code is wiped on successful exchange and on expired-detection. Storage is single-slot (`Option<PairingCode>`), so generating a new code overwrites the previous one.
- **Generation:** 6-digit zero-padded numeric (`format!("{:06}", rng.gen_range(0..1_000_000))`) via `rand::thread_rng()`.
- **Wrong-code attempts during the 5-min window do not invalidate the code** — only expiration or successful exchange does. Implication: typos are non-fatal until the window closes.

**Note re #722.** The pairing-code generator currently uses `rand::thread_rng()`, but #722 ("Tech debt: Switch auth.rs from thread_rng to OsRng (Send-safe)") is filed launch-blocking and security-critical. The capture in next turn's deliverable will document the post-#722 form (OsRng) as the spec'd behavior, with a note explaining the present transitional state and citing #722. This avoids the capture going stale the moment #722 lands.

---

## New gap surfaced (not in instructions doc enumeration)

The instructions list OpenRouter `/auth/key` and Anthropic `/v1/models` as "captured in dev dispatch X5." X5 contains **prose descriptions** of those endpoints' behavior ("returns key validity + credit balance in one call"), not actual response shapes.

Per ADR-0023, prose descriptions are not captures. Locking the spec without actual response shapes is the same authoring pattern that produced §22C's three-convention drift (§826 audit). Per Matt's robustness stance, papering over isn't an option.

**Plan:** `web_fetch` OpenRouter + Anthropic public docs in next turn's captures step. Public docs, no API credits consumed. The capture file will:

- Lift the actual JSON response shapes from each provider's documentation
- Cite the doc URL + access date (ADR-0023's source-of-truth requirement)
- Note the trust level per ADR-0023's versioning policy (vendor docs are typically less reliable than source-code or live-probe — flag accordingly)
- Mark Anthropic's known billing-error misclassification quirk (X5: 400 `invalid_request_error` with "credit balance is too low" in message — bites every Anthropic call, not just onboarding; sub-task #1 already filed)

If web docs prove insufficient (e.g., Anthropic doesn't document `/v1/models` response in detail publicly), the fallback is a live curl with a test API key — but that requires Matt to share a low-privilege key, which is its own privacy question. Flagging in advance so we can switch to that path quickly if needed.

---

## Decisions and clarifications worth surfacing

**Date convention.** Today is 2026-05-07. ADRs use this date.

**Author line.** Match ADR-0023 convention: "Matt Carano, Claude (spec session)". The ADRs are joint output — Matt's locked decisions, drafted in this session.

**Status line.** All three ADRs are `Accepted`. The instructions describe T1/T2/T3 as "locked" — that's `Accepted` per the playbook's standard terms.

**Cross-citations between the three ADRs.** ADR-0024 (decryption) is independent of ADR-0025 (check cadence) and ADR-0026 (identifier choice) at the data layer, but the three reference each other to make the relationships clear: ADR-0024 says "ecom is for entitlement only," ADR-0025 says "here's when entitlement is checked," ADR-0026 says "here's what's checked in `active_machine_id`."

**Sub-task references.** Each ADR cites the launch-blocking sub-tasks it depends on (#5 backup format, #6 ecom enforcement, #7 gateway pairing rejection, #16 help copy). This gives Brit's dispatch-management direct visibility on the implementation chain.

**Tier 1 forward-compat language.** Each ADR explicitly bounds itself to Tier 0 (single-machine, local-first). Tier 1 hosted introduces server-side encryption (per ADR-0004), different trust roots, and multi-machine support. The ADRs are written to *not* preclude Tier 1, and to make the bounds explicit.

---

## Sequencing followed (vs. instructions)

1. ✅ Read dev dispatch + Reed's audit + existing `docs/spec/onboarding.md` §6 (turn 1).
2. ✅ `project_knowledge_search` for highest existing ADR number → 0023, confirmed (turn 1).
3. ✅ Capture Ollama + OC external contracts BEFORE drafting spec (turn 1 — Reed's existing capture lifted; my source-reads closed the two flagged gaps; OR + Anthropic captures done in turn 2 via `web_fetch`).
4. ✅ Draft 3 ADRs (turn 1; ADR-0026 nit fixed in turn 2).
5. ✅ Rewrite §6 with sub-structure per audit (turn 2).
6. ✅ Add §22C.22 onboarding integration sub-section (turn 2).
7. ✅ Propagate §22C.11 update + §22B retirement markers + §24.5 update + README.md §0.3.2 update (turn 2).
8. ✅ Cross-refs and ADR cross-citations within the spec (handled inline as each section drafted).

---

## What didn't ship (and why)

**Final pass for cross-refs across ALL files at session end** — handled inline as each section drafted, not as a separate pass. Each spec file's references to other spec files / ADRs / captures use the new ADR numbers and the new §22C.22 number. If Brit finds a stale reference during PR review, it's a missed inline catch, not a deferred task.

**OR `/api/v1/models` shape capture (capture §4.4)** — RESOLVED. Reed's capture at `docs/audits/2026-05-07-external-contract-capture-openrouter-models.md` closed the gap; lifted into `01-external-contract-captures.md` §4.4 with the pricing-strings-not-numbers detail flagged for adapter implementations. No longer outstanding.

**Anthropic billing-error misclassification handler (sub-task #1)** — referenced inline in §6.3.2 and capture §6 as launch-blocking, but the handler implementation isn't part of the spec session. The spec captures the requirement; sub-task #1 is the implementation.

**OR endpoint path verification** — RESOLVED. Brit confirmed shipped code uses `/api/v1/auth/key` (`src-tauri/src/system_files.rs:628`); spec aligned to shipped-code path. Vendor-docs alignment (whether `/api/v1/auth/key` is a documented alias or a deprecated duplicate of vendor's `/api/v1/key`) is a separate side issue, not blocking spec-lock.

---

## Outputs in this session

In `/mnt/user-data/outputs/onboarding-spec-rewrite-2026-05-07/`:

```
00-session-notes.md                                       (this file)
01-external-contract-captures.md                          (lift to docs/spec-prep/2026-05-07-external-contract-captures.md)
02-spec-onboarding-section-6.md                           (drop into docs/spec/onboarding.md, replacing §6)
03-spec-sync-section-22C-22-onboarding-integration.md     (drop into docs/spec/sync.md as new §22C.22)
04-spec-sync-section-22C-11-update.md                     (apply two changes to docs/spec/sync.md §22C.11)
05-spec-sync-22B-retirement-banners.md                    (apply three banners to docs/spec/sync.md §22B.2/22B.3/22B.8A)
06-spec-agents-section-24-5-update.md                     (replace docs/spec/agents.md §24.5 body)
07-spec-readme-section-0-3-2-update.md                    (apply diagram + subsection edits to README.md §0.3.2)
0024-local-only-decryption.md                             (drop into docs/adr/)
0025-ecom-check-cadence.md                                (drop into docs/adr/)
0026-install-uuid-active-machine-id.md                    (drop into docs/adr/)
```

ADR target paths: `docs/adr/0024-…md`, etc. Spec target paths called out per file.

---

## Unblocked / blocked items for Brit

**Unblocked by this session's spec lock:**

- Sub-tasks #41 (ADR-track decisions) — three ADRs landed; state-machine implementation can proceed.
- Sub-tasks #879 / #880 / #881 — onboarding flow specs locked; implementation can be scoped.
- Sub-tasks #883 / #275 — Your Team display + Settings differentiation specs locked.
- Sub-task #884 — concrete instructions for token sources locked in §6.5.3.
- **Sub-task #8 — RESOLVED.** Brit's gateway source-read confirmed: no SameMachine bypass exists. Verified across four candidate paths (env-var override, sideload, loopback exemption, middleware fall-through); none found. Full pairing required regardless of topology. §22C.22.3 + §6.5.2 collapsed to single-path narrative.

**Still blocked / pending:**

- Sub-task #5 (backup file format) — launch-blocking and a dependency of ADR-0024 + ADR-0026. Verify whether backup includes encrypted provider keys + `install-uuid.json`.
- Sub-task #722 — `thread_rng` → `OsRng` migration. Spec is written against post-#722 form; capture §3 documents the transitional state.

**New launch-blocking issues surfaced this session (not part of this spec scope):**

- **Settings-side SameMachine auto-discovery shipped-broken.** PR #885 shipped `~/.openclaw/openclaw.json` auto-discovery for the Settings UI (§22C.11A.1) but the implementation is broken in the live build. Separate launch-blocking issue being filed; cleanup PR will fix the shipped behavior to match the §22C.11.5 spec (URL + config sanity-check, not credential extraction). Out of scope for this redesign session per Matt's directive.
- **Plaintext API keys in DB.** BYOK provider keys stored plaintext in `ai_settings` rows, not in keychain (confirmed via `backup.rs` source-read). ADR-0024 references this stance; full security implications and remediation tracked as a separate issue.
- **Pairing-code wrong-attempt rate-limiting.** No rate-limiting on wrong-code attempts at `/pair`. Capture §3 acknowledges honestly; mitigation beyond network-layer trust is a separate side issue.

**Index of spec cross-references that landed in this session** (for Brit's PR review):

- ADR-0024 ↔ §6.1.2, §6.1.4, §22C.22 (decryption stance, lapse UI, backup-format dependency)
- ADR-0025 ↔ §6.1.2, §6.1.3 (cadence for subscription + take-over checks)
- ADR-0026 ↔ §6.1.3, §6.1.4 (`install_uuid` field, backup-format dependency)
- §22C.22 ↔ §6.5 (every screen subsection), §22C.5, §22C.11, §22C.11A.5, §22C.4.3, §22C.6.2, §22C.16
- §22C.11.5 (new) ↔ §22C.11A.1, §22C.11A.6, §6.5.1, §6.5.2, §22C.22.3
- §22B.2 / §22B.3 / §22B.8A retirement banners ↔ §22C.6.2, §22C.5.1, §22C.4.3, §22C.16, §22C.22
- §24.5 ↔ §22C.6.2, §22C.22, §6.5.4 whitelist, §6.5.5 Your Team, ADR-0007, ADR-0024
- README §0.3.2 ↔ §22C.16, §22C.22, §6.5
- External-contract captures ↔ §22C.5.1, §22C.4.3, §6.3.1, §6.3.2, §6.3.3, §6.5.3
