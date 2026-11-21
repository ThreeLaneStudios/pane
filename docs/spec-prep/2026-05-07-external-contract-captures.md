# External-Contract Captures — Onboarding (#953)

**Capture session:** 2026-05-07
**Format:** Per ADR-0023 + dev-hygiene-playbook §1
**Scope:** Every external system the rewritten §6 onboarding spec invokes. Each capture pins source-of-truth, version, and a versioning note.

This file is the single canonical reference for external response shapes the onboarding flow depends on. Code that adapts each shape references the relevant section here; this section references the source-of-truth (vendor doc URL, source-code path, or live-probe artifact).

Sources of truth, in trust order: source code (most reliable) → vendor docs (intermediate; vendor doc lag from current API behavior is real) → live probe (point-in-time).

---

## Capture index

| # | Endpoint | Path | Method | Source-of-truth | Trust |
|---|---|---|---|---|---|
| 1 | OpenClaw gateway pairing | `POST /pair` | POST | Pane gateway source code | High (own code) |
| 2 | OpenClaw gateway pairing — error wire | `POST /pair` (error path) | POST | Pane gateway source code + integration tests | High (test-pinned) |
| 3 | OpenClaw gateway pairing-code internals | (server-side, not a Pane call site) | n/a | `src-gateway/src/auth.rs` source-read | High (own code) |
| 4 | OpenRouter — current API key | `GET /api/v1/auth/key` | GET | openrouter.ai/docs | Medium (vendor docs) |
| 5 | OpenRouter — list models | `GET /api/v1/models` | GET | Reed's capture, `docs/audits/2026-05-07-external-contract-capture-openrouter-models.md` | Medium (vendor docs via Reed) |
| 6 | Anthropic — list models | `GET /v1/models` | GET | docs.anthropic.com/docs.claude.com | Medium (vendor docs) |
| 7 | Anthropic — error envelope | (response on any error) | n/a | docs.anthropic.com/api/errors | High (well-documented + observed in wild) |
| 8 | Ollama — list local models | `GET /api/tags` | GET | Live probe (Reed) | High (live-probe, version-pinned) |

---

## 1. OpenClaw Gateway — `POST /pair` success path

**Source:** Pane gateway source code, commit `1f3af69918966cc83e4955d5a668d329a9b213fc` (captured 2026-05-07 by Reed).
**Versioning:** Internal sibling service. Pane and gateway are authored together; this is structurally a sibling-service contract, not an external one. Captured here for completeness because the onboarding flow consumes it without seeing the source.

**Files cited:**
- Route definition: `src-gateway/src/server.rs:115`
- Handler: `src-gateway/src/routes/pair.rs:8-23`
- Request shape: `src-gateway/src/auth.rs:22-25`
- Response shape: `src-gateway/src/auth.rs:28-34`

### Request shape (`PairRequest`)

```json
{ "code": "<6-digit pairing code>" }
```

`code` is required. Single field. JSON deserialization is strict via serde.

### Response shape on 200 (`PairResponse`)

```json
{
  "token": "<256-bit hex string, 64 chars>",
  "expires_at": "<RFC3339 timestamp, 90 days from issue>",
  "tenant_id": "default",
  "gateway_version": "0.1.0"
}
```

- `token` — opaque bearer (256-bit, hex-encoded). Stored to keychain by Pane (per §22C.5.1).
- `expires_at` — RFC3339; tokens expire 90 days after issue. Rotation policy in §22C.5.1.
- `tenant_id` — always `"default"` at launch (single-tenant per ADR-0013); seam in place for Tier 1.
- `gateway_version` — currently hardcoded `"0.1.0"` in source. Pane uses this for client/server-version diagnostics, not feature gating, at launch.

---

## 2. OpenClaw Gateway — `POST /pair` error wire shape

**Source:** Pane gateway source code at the same commit as Capture 1.
**Versioning:** Internal sibling service. Test-pinned: integration tests in `src-gateway/tests/error_classification_test.rs` assert each error variant's wire body, so the snake_case `error` field values cannot drift silently — a drift fails the test suite at PR time.

**Files cited:**
- Error envelope: `src-gateway/src/error.rs:10-18` (the `ErrorResponse` struct)
- Variant→HTTP mapping + wire body: `src-gateway/src/error.rs:78-195` (the `IntoResponse` impl)
- Pairing-handler error path: `src-gateway/src/routes/pair.rs:15` (delegates to `exchange_code_for_token` which returns `Result<TokenInfo, GatewayError>`)
- Wire-body assertions: `src-gateway/tests/error_classification_test.rs` (one test per variant; asserts both HTTP status and `body["error"]` string)

### Error envelope (all gateway errors)

```json
{
  "error": "<snake_case_error_code>",
  "status": <u16 HTTP status>,
  "message": "<human-readable string>",
  "retry_after": <u64 seconds, only on 429/503>,
  "max_bytes": <usize, only on 413>
}
```

`retry_after` and `max_bytes` use `#[serde(skip_serializing_if = "Option::is_none")]` and are absent from the JSON when not applicable (not present-as-`null`).

### Variants reachable via `POST /pair`

The pairing handler invokes only `exchange_code_for_token`. Source-read of `auth.rs` (see Capture 3) shows three reachable failure modes, all mapped to `GatewayError::Unauthorized(_)`. Axum-level deserialization failures upstream of the handler produce `GatewayError::InvalidRequest(_)`.

| Wire `error` | HTTP | Reachable on `/pair` | Wire `message` (verbatim from source) |
|---|---|---|---|
| `"unauthorized"` | 401 | Pairing code expired (5-min window elapsed before exchange) | `"Pairing code expired"` |
| `"unauthorized"` | 401 | Pairing code wrong (active code present but doesn't match) | `"Invalid pairing code"` |
| `"unauthorized"` | 401 | No active pairing code (none generated, or last one was already exchanged or expired-and-cleared) | `"No active pairing code"` |
| `"invalid_request"` | 400 | Body deserialization failure (missing `code`, malformed JSON) | (axum/serde error string) |

Pane's UX must distinguish the three "unauthorized" sub-cases by `message` substring, since they map to different recovery flows (regenerate code on the OC side vs. retry the same code with the right typo). See §22C.22 for onboarding error-table mapping.

### Variants reachable via OTHER gateway endpoints (not `/pair`, listed for completeness)

Pane sees these on subsequent gateway calls (initial sync, model list, chat). Not pairing-time, but onboarding flow continues into them on success of `/pair`.

| Wire `error` | HTTP | When |
|---|---|---|
| `"not_implemented"` | 501 | Endpoint not implemented yet |
| `"config_error"` | 500 | Gateway misconfigured |
| `"tls_error"` | 500 | TLS subsystem failure |
| `"io_error"` | 500 | I/O failure |
| `"not_found"` | 404 | Resource not found |
| `"payload_too_large"` | 413 | Request body exceeds cap (10 MiB push, 1 MiB session message) — includes `max_bytes` |
| `"oc_unauthorized"` | 502 | Gateway cannot authenticate to OC |
| `"oc_forbidden"` | 502 | OC rejected gateway request |
| `"oc_rate_limited"` | 429 | OC rate-limited the gateway — includes `retry_after` |
| `"oc_server_error"` | 502 | OC internal server error |
| `"oc_unavailable"` | 503 | OC service unavailable — includes `retry_after` |
| `"oc_timeout"` | 504 | Gateway timed out waiting for OC |
| `"oc_connection_lost"` | 502 | OC WebSocket dropped |
| `"oc_client_error"` | 502 | Generic OC client error with message |

---

## 3. OpenClaw Gateway — pairing-code internals (`exchange_code_for_token`)

**Source:** `src-gateway/src/auth.rs` — `generate_pairing_code` and `exchange_code_for_token` functions, same commit as Captures 1–2.
**Versioning:** Internal sibling service. Captured because §22C.5.1 currently summarizes only the customer-visible properties; the implementation details below close gaps that block onboarding spec-locking.

### Generation properties

- **Format:** 6-digit zero-padded numeric (`0` through `999999`, displayed as `000000`–`999999`).
- **Source code:** `format!("{:06}", rng.gen_range(0..1_000_000))`.
- **RNG:**
  - **Current (pre-#722):** `rand::thread_rng()` — ChaCha12-backed thread-local RNG. Not the OS CSPRNG.
  - **Post-#722 (canonical for spec):** `OsRng`. Issue #722 ("Tech debt: Switch auth.rs from thread_rng to OsRng (Send-safe)") is filed launch-blocking, security-critical, and tracked in roadmap #536. **The §22C / §6 spec is written against the post-#722 form.** This avoids the spec going stale the day #722 lands and makes the security-relevant intent explicit. If this capture is read before #722 lands, treat the live behavior as transitional.
- **Entropy:** ~20 bits (1,000,000 possible codes). The 5-min TTL caps the attack window per code, but **wrong-code attempts do not consume the slot or trigger any rate-limiting** (see Lifecycle item 4 below — wrong-code attempts return 401 without mutating state). Within a single 5-min window, an attacker holding a network position to the gateway can submit unbounded guesses against the active code. Brute-force exposure is bounded by network-layer trust — for SameMachine, the gateway listens on `127.0.0.1` only (loopback-trusted); for SplitMachine, the gateway is reachable only over Tailscale-authed peers (Tailscale ACL is the trust boundary). The single-slot model prevents *parallel* code campaigns but not *sequential* guessing within one window. This is acknowledged honestly here; mitigations beyond network-layer trust are tracked as a separate side issue, not in scope for the redesign spec. *[Superseded, scope-2 PR: the SameMachine clause above is dead — that topology was retired (§22C.11, #2649). The SplitMachine clause is also superseded: the gateway now enforces a configurable remote-address allowlist at TCP accept (`docs/spec/sync.md` §22C.5) rather than relying on Tailscale ACLs, and the pairing-code endpoint is now rate-limited (~10 attempts per active code, lockout until the code's own expiry, 6- and 8-digit codes both covered) rather than unbounded within the 5-min window as described above. See §22C.5 and §22C.5.1 for current behavior.]*

### Lifecycle

1. **Single-slot storage.** Pairing-code state is `RwLock<Option<PairingCode>>` — at most one code is active at any moment. Generating a new code (e.g., customer ran `openclaw gateway pairing-code` again) overwrites the previous one in memory; the previous code becomes invalid immediately.
2. **TTL: 5 minutes.** `expires_at = Utc::now() + Duration::minutes(5)` at generation.
3. **One-time use.** On successful exchange, `*pairing_code = None` (the slot is cleared) before the token is returned. The same code cannot be exchanged twice. On exchange-with-expired-code, the slot is also cleared (post-condition: no expired code lingers).
4. **Wrong code does not invalidate.** If the slot holds an active (non-expired) code and the customer submits a different value, the gateway returns 401 `"unauthorized"` with message `"Invalid pairing code"` but **does not clear the slot**. The customer can retry with the correct code as long as the 5-min window remains open. This makes typos non-fatal — the spec'd UX (#880 "Verify and connect" screen) relies on this.

> **Test-coverage gap (filed as small follow-up).** The typo-tolerance contract above is not pinned by any existing test. `test_exchange_invalid_code` calls `exchange_code_for_token("999999")` against an empty slot, hitting the `None` arm — it does not exercise the `Some(_)` (wrong-code-with-active-slot) arm. The spec relies on the `Some(_)` arm not mutating `pairing_code`; a future refactor that accidentally adds `*pairing_code = None;` to that arm would silently flip the typo-tolerance contract and break the §6.5.3 / §22C.22.4 retry UX without failing any test. Source-read of `auth.rs` 2026-05-07 confirmed the current behavior matches the spec; recommendation is a 10-line follow-up test that generates a code, submits a wrong code, then submits the right code, and asserts success. Cite: capture session 2026-05-07 source-read of `src-gateway/src/auth.rs`.

### Token issuance

On successful exchange, the gateway returns a `TokenInfo`:
- `token`: 256 random bits (32 bytes from RNG), hex-encoded → 64-char string.
- `expires_at`: `Utc::now() + Duration::days(90)`.
- `tenant_id`: `"default"` at launch.

Token is then persisted to the gateway's `tokens` JSON file (atomic-write via tempfile + rename, 0o600 permissions) and held in `RwLock<HashMap<String, TokenInfo>>` for verification.

---

## 4. OpenRouter — `GET /api/v1/auth/key` (current key info)

**Source:** [OpenRouter API Reference — Get current API key](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key) (fetched 2026-05-07).
**Versioning:** Vendor docs. OpenRouter has historically been API-stable for this endpoint; treat as **stable** but re-verify each minor release per ADR-0023.

### Path discrepancy — resolved 2026-05-07

Three different path strings exist across the project's documentation and code:

| Source | Path | Status |
|---|---|---|
| Shipped code | `/api/v1/auth/key` | `src-tauri/src/system_files.rs:628` — what the production app actually calls |
| Vendor docs | `/api/v1/key` | What openrouter.ai publishes for "Get current API key" |
| Dev dispatch X5 | `/auth/key` | Shorthand reference in the redesign dispatch; not the literal string |

**Trust order: shipped code wins.** The spec uses `/api/v1/auth/key` because that is what the running adapter sends to OpenRouter, and OR is currently accepting that request shape (the OR validation flow ships and works). The discrepancy with vendor docs may indicate (a) `/api/v1/auth/key` is a stable alias OR maintains alongside the documented `/api/v1/key`, (b) the docs are stale, or (c) Pane is calling a deprecated path that still works. Whichever is the case is a vendor-docs-alignment side issue tracked separately; it does not block onboarding spec-lock and does not affect the §6.3.1 customer-facing flow.

### Request

```
GET https://openrouter.ai/api/v1/auth/key
Authorization: Bearer <user's OR API key>
```

### Response on 200

```json
{
  "data": {
    "byok_usage": 17.38,
    "byok_usage_daily": 17.38,
    "byok_usage_monthly": 17.38,
    "byok_usage_weekly": 17.38,
    "creator_user_id": "user_2dHFtVWx2n56w6HkM0000000000",
    "include_byok_in_limit": false,
    "is_free_tier": false,
    "is_management_key": false,
    "label": "sk-or-v1-au7...890",
    "limit": 100,
    "limit_remaining": 74.5,
    "limit_reset": "monthly",
    "usage": 25.5,
    "usage_daily": 25.5,
    "usage_monthly": 25.5,
    "usage_weekly": 25.5,
    "is_provisioning_key": false,
    "rate_limit": {
      "interval": "1h",
      "note": "This field is deprecated and safe to ignore.",
      "requests": 1000
    },
    "expires_at": "2027-12-31T23:59:59Z"
  }
}
```

Fields material for onboarding (single source-of-truth for §6.3 BYOK validation logic):

- `data.label` — masked-key string. Confirms which key was authenticated; not used by Pane's UI at launch.
- `data.limit` — credit cap on this key (numeric, in OR credit units; can be `null` for unlimited keys).
- `data.limit_remaining` — credits remaining on the cap. **The credit-availability signal Pane uses for the "no credits" onboarding error.** A key with `limit=100, limit_remaining=0.0` is "no credits" for onboarding purposes.
- `data.usage` — credits used so far on this key.
- `data.is_free_tier` — boolean. If true, the key is on OR's free tier.
- `data.expires_at` — RFC3339 expiration timestamp. Currently informational at onboarding; future Settings UX could surface.

### Response on auth/credit failures

- **401 Unauthorized** — invalid key, malformed key, or revoked key. OR-side error envelope; spec-side is "Auth failed" UX (per §6.3).
- **402 Payment Required** — emitted when the key authenticated but credits cannot cover the request. Note: at the `/key` info endpoint, 402 is observed primarily when the account itself is suspended; the more typical "no credits" path is 200 with `limit_remaining=0` (see fields above). Pane's onboarding logic handles both paths.
- **5xx** — OR upstream issue. Surface as "Couldn't reach OpenRouter" with retry. Per X6, customer can also Skip to the unverified state.

### Retry/eventual-consistency note

Per X5, OR billing has eventual consistency. After a customer adds credits, immediate retry of `/api/v1/auth/key` may still report `limit_remaining=0`. UX must include the 30-second propagation hint (per design brief OR copy: "After adding credits, takes a moment to reflect — try again in 30 seconds.").

---

## 4.4 OpenRouter — `GET /api/v1/models` (model list, post-validation picker)

**Source:** Reed's contract capture, `docs/audits/2026-05-07-external-contract-capture-openrouter-models.md`. Captured against openrouter.ai vendor docs. Vendor-docs trust tier per ADR-0023; re-verify each pre-launch dogfood cycle.
**Versioning:** OpenAI-compatible models-list shape; OpenRouter has been stable here historically.

### Request

```
GET https://openrouter.ai/api/v1/models
```

**No auth required.** Unauthenticated endpoint — Pane fetches the catalog before the customer's key is validated, or even when the customer has no key yet. (This is what enables the "search models" preview surface to function before validation in some redesigns; for the v1 picker, fetch happens after Test Connection success per §6.3.1.)

### Response on 200

```json
{
  "data": [
    {
      "id": "anthropic/claude-3.5-sonnet",
      "name": "Claude 3.5 Sonnet",
      "created": 1234567890,
      "description": "Anthropic's Claude 3.5 Sonnet model",
      "context_length": 200000,
      "pricing": {
        "prompt": "0.000003",
        "completion": "0.000015"
      },
      "top_provider": {
        "max_completion_tokens": null,
        "is_moderated": false
      }
    }
  ]
}
```

Per-entry fields material for the picker:

- `id` — full provider/model identifier (e.g., `"anthropic/claude-3.5-sonnet"`). What Pane stores as the model selection. The `provider/` prefix is the substring-match anchor for the search-filter logic in sub-task #28.
- `name` — human-readable label for the picker UI.
- `description` — short prose; Pane may surface this as a hover tooltip or one-line caption (sub-task #28 finalizes).
- `context_length` — integer, tokens. Currently informational at onboarding; Settings post-launch could surface.
- `pricing.prompt` and `pricing.completion` — **string values, not numbers** (e.g., `"0.000003"` for $3 per million prompt tokens). Adapter implications: any code parsing pricing must handle string-to-decimal conversion; storing pricing in numeric form requires an explicit parse step. JSON `Number` deserialization will fail; deserialize into `String` and parse separately.
- `top_provider` — sub-object with `max_completion_tokens` (integer or null) and `is_moderated` (boolean).

The `created` field is a Unix timestamp; useful for "recently released first" sort if implemented.

### Errors

OR's standard error shape applies — 5xx for upstream issues, network errors. No 401 expected on this endpoint (no auth required); a 401 would be unexpected.

### Adapter notes

- **Pricing as strings is load-bearing.** Code that consumes this response must not assume numeric deserialization. The 2026-05-02 BYOK shape audit (Reed) flagged this same shape; sub-task #28 (search/filter logic) and #38 (shared picker component) inherit this constraint.
- **Catalog size.** OR's catalog is on the order of 100–200 models, large enough that the picker's search-filter is load-bearing for usability. The picker fetches once per onboarding session and caches in memory; no pagination needed at the response level.
- **Auth-not-required interaction with credit-check.** The picker fetch and the `/api/v1/auth/key` validation are separate calls with separate auth requirements. Pane's flow runs auth-key validation first (§6.3.1 step 2), then models-list fetch (step 3); the order is UX-driven, not auth-driven.

---

## 5. Anthropic — `GET /v1/models`

**Source:** [Anthropic API Reference — List Models](https://docs.anthropic.com/en/api/models-list) (fetched 2026-05-07; same content mirrored at `docs.claude.com/de/api/models-list`).
**Versioning:** Vendor docs, with `anthropic-version` header pinning. Treat as **stable** within a documented `anthropic-version`. The `2023-06-01` version has been stable for years; new model entries appear on schedule but the response shape itself does not break under that version pin.

### Request

```
GET https://api.anthropic.com/v1/models
x-api-key: <user's Anthropic API key>
anthropic-version: 2023-06-01
```

Note: **Anthropic uses `x-api-key` header, not `Authorization: Bearer`.** This differs from OR. Pane's BYOK key entry adapter must conditionally select header style per provider.

Optional pagination params (query string): `before_id`, `after_id`, `limit` (default 20, range 1–1000).

### Response on 200

```json
{
  "data": [
    {
      "type": "model",
      "id": "claude-sonnet-4-20250514",
      "display_name": "Claude Sonnet 4",
      "created_at": "2025-02-19T00:00:00Z"
    }
  ],
  "first_id": "<string>",
  "has_more": true,
  "last_id": "<string>"
}
```

Per-entry fields (all required when the entry is present):

- `type` — always `"model"`. (Note: this is `type`, not `object` — distinct from the OpenAI/OR convention; cross-cited issue: BerriAI/litellm#27180, open-webui/open-webui#18929 both flagged the difference for adapter implementers.)
- `id` — full versioned model identifier (e.g., `"claude-sonnet-4-20250514"`). What Pane stores as the model selection.
- `display_name` — human-readable label for the picker UI (e.g., `"Claude Sonnet 4"`).
- `created_at` — ISO 8601 timestamp. Not surfaced to user; useful for "recently released first" sort if added later.

Top-level pagination fields: `first_id`, `last_id`, `has_more` — surface a cursor model for fetching multiple pages. For onboarding's purposes (≤20–25 models, easily fits on one page at default limit), Pane fetches the first page and reads `has_more`; if `has_more=true`, paginates with `after_id=last_id` and concatenates.

### Errors

Use the Anthropic error envelope (Capture 7). Key error types reachable from `/v1/models`:

- **401 `authentication_error`** — invalid `x-api-key`. Surface as "Auth failed" (per §6.3).
- **403 `permission_error`** — key lacks permission. Treat same as auth-failed in onboarding UX — customer is told the key doesn't work and should get a new one.
- **429 `rate_limit_error`** — rare on the `/v1/models` endpoint. Surface as the design brief's rate-limit copy.
- **5xx** — surface as "Couldn't reach Anthropic" with retry.

### Credit-validation gap (acknowledged)

Per X5: `/v1/models` does NOT validate billing. A customer with a working key and zero credits can complete §6.3 successfully and only discover the credit issue on their first chat. This is an accepted onboarding limitation (sub-task #1 covers the first-chat misclassification; sub-task #32 covers verifying clean error surface).

---

## 6. Anthropic — error envelope (used by `/v1/models` and every other call)

**Source:** [Anthropic API Reference — Errors](https://docs.anthropic.com/en/api/errors) (fetched 2026-05-07). Cross-confirmed via [`anthropic-sdk-go` error-handling docs](https://deepwiki.com/anthropics/anthropic-sdk-go/3.4-error-handling) and observed-in-wild GitHub issues.
**Versioning:** Vendor docs note "the values within these objects may expand, and it is possible that the type values will grow over time." Treat as **stable on the field shape, growing on the type enumeration.** Pane's adapter must accept unknown `error.type` values gracefully and treat them as the HTTP-status default.

### Wire shape

```json
{
  "type": "error",
  "error": {
    "type": "<error_type_string>",
    "message": "<human-readable string>"
  },
  "request_id": "<req_xxx>"
}
```

### Type → HTTP mapping

| `error.type` | HTTP | Notes |
|---|---|---|
| `invalid_request_error` | 400 | Format/content issue. **Quirk:** also used for billing failures sometimes (see below). |
| `authentication_error` | 401 | API key issue. |
| `billing_error` | 402 | Billing/payment issue. **Sometimes returned as 400 `invalid_request_error` instead — see quirk below.** |
| `permission_error` | 403 | Key lacks permission. |
| `not_found_error` | 404 | Resource not found. |
| `request_too_large` | 413 | >32 MB request body. Cloudflare-level. |
| `rate_limit_error` | 429 | Rate-limited. |
| `api_error` | 500 | Anthropic-side internal error. |
| `timeout_error` | 504 | Request timeout. |

### The billing-error misclassification quirk (X5)

Anthropic sometimes returns billing failures as `400` with `error.type = "invalid_request_error"` and a message containing the substring `"credit balance is too low"`, instead of the documented `402 billing_error`. This bites every Anthropic call, not just onboarding. Onboarding-relevant: at first-chat (the place the customer hits the misclassification, since onboarding's `/v1/models` validation doesn't probe billing), Pane must classify the billing-error correctly to surface "no credits" rather than "invalid request."

Detection rule (per sub-task #1, applies wherever Pane handles Anthropic errors):

```
if response.status == 400 
   and body.error.type == "invalid_request_error"
   and "credit balance" in body.error.message.lower():
    treat as billing_error
```

Filed as launch-blocking. Cross-reference: sub-task #32 verifies clean billing-error surface on first chat after onboarding.

---

## 7. Ollama — `GET /api/tags` (local model list)

**Source:** Reed live-curl probe, captured 2026-05-07 against Ollama 0.20.7 running at `http://localhost:11434`. Verbatim record in `2026-05-07-external-contract-captures-ollama-pairing.md` (Reed). Re-confirmed against Ollama's official docs 2026-05-19 (`https://docs.ollama.com/api/tags`); shape stable.
**Versioning:** **Stable.** Ollama's `/api/tags` shape has been stable across multiple minor releases. Re-verify each Ollama major version bump.
**Usage in Pane:**
- **§6.3.3 (primary BYOK Ollama):** silent localhost probe at picker click; model list populates the §6.3.3 model picker.
- **§6.4.4 (Smart Features internal-provider Ollama):** second of two presence-probe calls (after `/api/version`, §8); confirms at least one model is available for internal-provider use. Same wire shape, same response handling — Pane does not display a model picker at the §6.4.4 step (first available model is auto-selected); the per-tier model picker lives in Settings → AI tab → Smart Features section per ADR-0037.

### Request

```
GET http://localhost:11434/api/tags
```

No auth (local-only by default).

### Response headers (observed)

```
HTTP/1.1 200 OK
Content-Type: application/json; charset=utf-8
```

### Response body shape

```json
{
  "models": [
    {
      "name": "llama3.2:3b",
      "model": "llama3.2:3b",
      "modified_at": "2026-04-15T14:18:21.799779027-04:00",
      "size": 2019393189,
      "digest": "a80c4f17acd55265feec403c7aef86be0c25983ab279d83f3bcd3abbcb5b8b72",
      "details": {
        "parent_model": "",
        "format": "gguf",
        "family": "llama",
        "families": ["llama"],
        "parameter_size": "3.2B",
        "quantization_level": "Q4_K_M"
      }
    }
  ]
}
```

Empty `models: []` is a valid response — represents "Ollama is running but no models pulled yet." §6.3.3 specifies the "No models installed" empty state in that case.

### Connection details (Reed's curl record)

```
* IPv6: ::1   → connection refused
* IPv4: 127.0.0.1 → connected
```

Note: IPv6 is refused, IPv4 succeeds. Pane's probe must handle both happy-path (IPv4 succeeds, IPv6 refused) and full-failure (both refused = Ollama not running). Per §6.3.3.

### Error states (from §6.3.3 / OL3 design brief)

- **Connection refused (Ollama not running)** → "Couldn't find Ollama at localhost:11434." UX path: install help, custom endpoint, retry.
- **200 with `models: []`** → "No models installed." UX path: Pane-hosted help with `ollama pull <name>` command + link to `ollama.com/library`.
- **200 with non-empty `models`** → advance to model picker.

### Field usage in onboarding

- `name` — what Pane displays after `:latest` stripping (per sub-task #39).
- `model` — internal model ID for chat API calls (preserved verbatim).
- Other fields not used at onboarding; `details` may be surfaced in Settings post-launch.

---

## 8. Ollama — `GET /api/version` (presence probe)

**Source:** Ollama official API docs, `https://docs.ollama.com/api-reference/get-version`, fetched 2026-05-19. Cross-confirmed against the Ollama Go client library types in `https://pkg.go.dev/github.com/ollama/ollama/api`.
**Versioning:** **Stable.** The `/api/version` endpoint returns a single `version` string and has been stable across Ollama releases. Re-verify each Ollama major version bump.
**Usage in Pane:** **§6.4.4 (Smart Features internal-provider Ollama):** first of two presence-probe calls. Lightweight "is Ollama reachable" check. Pane does not display the version to the customer; the response presence (200 OK + parseable body) is the signal. Failure on this call surfaces "Couldn't find Ollama at `<endpoint>`" per §6.4.4 outcomes table.

### Request

```
GET <endpoint>/api/version
```
Default `<endpoint>` is `http://localhost:11434`; customer can override for remote Ollama per §6.4.4. No auth.

### Response on 200

```json
{
  "version": "0.12.6"
}
```

Single field, `version`, string. Format is Ollama's release version (semver-ish; observed values include `0.12.6`, `0.20.7`).

### Response handling

Pane validates only that:

- HTTP status is 200, AND
- Response body parses as JSON with a non-empty `version` string field.

The version value itself is not used at onboarding. Pane could log it for support-triage purposes (post-launch consideration), but the onboarding flow does not gate on a minimum Ollama version. Rationale: Ollama's API surface is backwards-compatible enough that a version floor would be more friction than benefit; if a future Ollama release breaks `/api/tags` shape, the §6.4.4 failure mode for "couldn't list models" surfaces clearly regardless.

### Error states

- **Connection refused (Ollama not running)** → §6.4.4 surfaces "Couldn't find Ollama at `<endpoint>`. Make sure Ollama is running. [Try again] [Help with Ollama →]"
- **Non-200 status** → treat same as connection refused for §6.4.4 purposes; no separate retry-with-different-shape branch.
- **Body parse error** → treat same as connection refused; surface "Couldn't find Ollama at `<endpoint>`. Make sure Ollama is running."

The Ollama endpoint is local-by-default and the customer's responsibility to operate; Pane does not need to distinguish "version endpoint returned unexpected shape" from "Ollama not running" at the onboarding surface — both route to the same recovery path.

### Why two calls instead of one

§6.4.4 probes both `/api/version` (this section) and `/api/tags` (§7) before declaring the Ollama internal-provider configured. Why not just `/api/tags`:

- `/api/tags` returning `models: []` is "Ollama running, no models installed" — recoverable via `ollama pull`.
- `/api/tags` failing with connection refused is "Ollama not running" — recoverable via starting Ollama.
- A single-call probe to `/api/tags` could not distinguish "Ollama listening on a different port" from "Ollama down" without additional inference.

`/api/version` is the cheap "Ollama is there" signal; `/api/tags` is the cheap "Ollama has models" signal. Two calls, two distinct recovery messages, no inference layer. Cost is one extra HTTP call (both responses are small).

---

## Versioning & re-verify schedule

| Capture | Source-of-truth trust | Re-verify cadence |
|---|---|---|
| 1, 2, 3 (gateway pair) | Own source code | Every gateway-side PR that touches `src-gateway/src/auth.rs` or `error.rs`. CI fixture-deserialization test would mechanize per ADR-0023 future-work. |
| 4 (OR `/api/v1/auth/key`) | Vendor docs | Each pre-launch dogfood cycle; OR API has been stable historically. |
| 4.4 (OR `/api/v1/models`) | Reed's capture (vendor docs) | Each pre-launch dogfood cycle. |
| 5 (Anthropic `/v1/models`) | Vendor docs | Each Anthropic version bump that Pane adopts. `anthropic-version: 2023-06-01` is currently pinned. |
| 6 (Anthropic errors) | Vendor docs | Same as 5. Type enumeration may grow; field shape stable. |
| 7 (Ollama `/api/tags`) | Live probe | Each Ollama major version bump. |

---

## Outstanding follow-ups

1. **Brit (resolved 2026-05-07):** OR-key-validation path discrepancy resolved — shipped code uses `/api/v1/auth/key` (`system_files.rs:628`); spec aligned to shipped-code path. Vendor-docs alignment (whether OR's `/api/v1/auth/key` is a documented alias or a deprecated duplicate of `/api/v1/key`) is tracked as a separate side issue, not blocking spec-lock.
2. **Reed (resolved 2026-05-07):** OR `/api/v1/models` response shape captured at `docs/audits/2026-05-07-external-contract-capture-openrouter-models.md`; lifted into §4.4 above. Pricing-fields-as-strings detail flagged for adapter implementations.
3. **Tracking #722:** When the `thread_rng` → `OsRng` migration in `auth.rs` lands, this capture (Capture 3) is correct as written. If the migration slips past launch, file a spec amendment to flag the transitional state.
4. **Pairing-code typo-tolerance test (small follow-up):** Add a 10-line test that generates a code, submits a wrong code, then submits the right code, and asserts success. Pins the typo-tolerance contract that §6.5.3 / §22C.22.4 retry UX depends on. Source-read 2026-05-07 confirmed current behavior matches the spec; the test prevents future refactors from silently flipping the contract. Cross-reference: capture §3 test-gap note.
5. **CI fixture (post-launch / future-work per ADR-0023):** Captures 1, 2, 5, 6, 7 are candidates for a fixture-deserialization test that fails CI when the captured shape no longer matches the expected struct.

### Out of scope for this capture session

The following are tracked as separate side issues, surfaced during this session but not addressed:

- **Plaintext API keys in DB** — BYOK provider keys are stored plaintext in `ai_settings` rows, not in the OS keychain. ADR-0024 references this stance; full security implications and remediation tracked separately.
- **Pairing-code rate-limiting** — the gateway's `/pair` endpoint has no rate-limiting on wrong-code attempts (capture §3 entropy framing acknowledges this honestly). Network-layer trust is the bound; gateway-side rate-limiting is a separate side issue.
- **Settings-side SameMachine auto-discovery shipped-broken** — PR #885 shipped the `~/.openclaw/openclaw.json` auto-discovery for Settings (§22C.11A.1), but the implementation is broken (separate launch-blocking issue being filed). The §22C.11.5 spec (file 04) describes the *intended* shape; the cleanup PR brings the implementation in line.
- **OR vendor-docs alignment** — `/api/v1/auth/key` (shipped-code path) vs. `/api/v1/key` (vendor-docs path). Tracked separately; does not block spec-lock.
