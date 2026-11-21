# Supabase Auth API — Platform Contract Capture

**Capture Date:** 2026-05-07
**Capture Format:** ADR-0023 (third-party, real source-of-truth capture)
**Author:** Matt Carano, Claude (spec session)
**Tags:** #973, spec-prep, ecom-platform
**Locking ADR:** `docs/adr/0028-ecom-platform-selection.md`
**Used By:** ecom-backend wrapper (per ADR-0027)

---

## Format note

This is a **real ADR-0023 capture** (not the sibling-service exception). The wrapper code authored against this capture is the Pane organization's; Supabase Auth is the third-party. Reed's PR review on the wrapper repo verifies wrapper code against these shapes; stale captures are flagged for re-verification per ADR-0023's versioning policy.

Distinct from `docs/spec-prep/2026-05-07-pane-ecom-contract.md` (which is define-not-capture, sibling-service exception, governs Pane↔wrapper). Both apply.

## Source of truth

- **Primary docs:** https://supabase.com/docs/guides/auth (accessed 2026-05-07)
- **API reference:** https://supabase.com/docs/reference/javascript/auth-signinwithpassword (accessed 2026-05-07)
- **JWT structure:** https://supabase.com/docs/guides/auth/jwts (accessed 2026-05-07)
- **User management:** https://supabase.com/docs/guides/auth/managing-user-data (accessed 2026-05-07)
- **API keys:** https://supabase.com/docs/guides/getting-started/api-keys (accessed 2026-05-07)
- **Source repo (auth/gotrue):** https://github.com/supabase/auth (accessed 2026-05-07)

## Versioning note

**Stability:** semver-stable. Per Supabase Auth's own documentation: "Removal of JWT fields after a deprecation notice. Removal of certain APIs after a deprecation notice... We aim to provide a deprecation notice in execution logs for at least two major version releases or two weeks if multiple releases go out."

**Re-verification cadence per ADR-0023:** pre-launch (mandatory) + quarterly thereafter, OR on observed deprecation notices in execution logs / Supabase changelog.

**Trust level:** medium-high. Source code accessible (open-source); docs pages accurate against source; backward-compat commitment is explicit.

---

## Endpoints used by the wrapper

The wrapper integrates with Supabase Auth as a server-side admin client (service role key) for user lookups and metadata writes, and as a server-side user client for credential validation. Pane never communicates with Supabase Auth directly.

### 1. Sign in with password (validates credentials)

Used by wrapper's `/v1/auth/signin` to validate the incoming email+password.

**Path:** `POST /auth/v1/token?grant_type=password`
**Headers:**
```
apikey: <anon key>
Content-Type: application/json
```
**Request body:**
```json
{
  "email": "matt@example.com",
  "password": "..."
}
```
**Response (200):**
```json
{
  "access_token": "eyJhbGc...",
  "token_type": "bearer",
  "expires_in": 3600,
  "expires_at": 1715095440,
  "refresh_token": "...",
  "user": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "aud": "authenticated",
    "role": "authenticated",
    "email": "matt@example.com",
    "email_confirmed_at": "2026-05-07T12:00:00Z",
    "phone": "",
    "confirmed_at": "2026-05-07T12:00:00Z",
    "last_sign_in_at": "2026-05-07T12:00:00Z",
    "app_metadata": {
      "provider": "email",
      "providers": ["email"],
      "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
      "active_machine_name": "Matt's MacBook Pro",
      "subscription_status": "active",
      "subscription_lapsed_at": null,
      "account_first_name": "Matt"
    },
    "user_metadata": {},
    "identities": [...],
    "created_at": "2026-05-07T12:00:00Z",
    "updated_at": "2026-05-07T12:00:00Z"
  }
}
```

**Error responses:**
```json
// 400 — invalid credentials
{
  "error": "invalid_grant",
  "error_description": "Invalid login credentials"
}

// 400 — email not confirmed
{
  "error": "invalid_grant",
  "error_description": "Email not confirmed"
}

// 429 — rate limit (Supabase enforces internal rate limits)
```

**Wrapper behavior on response:**
- Discard `access_token` and `refresh_token` — wrapper does not pass these to Pane.
- Read `user.id`, `user.app_metadata` for entitlement state.
- Mint wrapper-issued opaque token, store mapping, return ADR-0027 §2 response shape to Pane.

---

### 2. Get user by email (existence check)

Used by wrapper's `/v1/auth/email-check`. Requires service role key.

**Path:** `GET /auth/v1/admin/users?email={email}`
**Headers:**
```
apikey: <service role key>
Authorization: Bearer <service role key>
```
**Response (200):**
```json
{
  "users": [
    { "id": "...", "email": "matt@example.com", ... }
  ],
  "aud": "authenticated"
}
```

If `users` array is empty, the email doesn't exist.

**Wrapper behavior:** translate to `{exists: bool}` and return per ADR-0027 §2.

**Note on enumeration:** ADR-0027 §2 acknowledges the user-enumeration oracle. Mitigation is at the wrapper layer (per-IP rate limiting on `/v1/auth/email-check`), not at the Supabase layer.

---

### 3. Update user app_metadata (admin write)

Used by wrapper for auto-claim on sign-in, take-over, and sign-out clearing. Service role only — this endpoint is the technical mechanism for the `app_metadata` privilege boundary.

**Path:** `PUT /auth/v1/admin/users/{user_id}`
**Headers:**
```
apikey: <service role key>
Authorization: Bearer <service role key>
Content-Type: application/json
```
**Request body:**
```json
{
  "app_metadata": {
    "active_machine_id": "550e8400-e29b-41d4-a716-446655440000",
    "active_machine_name": "Matt's MacBook Pro",
    "subscription_status": "active",
    "subscription_lapsed_at": null,
    "account_first_name": "Matt"
  }
}
```

**Response (200):** the updated `User` object (same shape as in §1 above).

**Critical discipline:** wrapper helpers MUST write to `app_metadata`, never `user_metadata`. Writing ecom-controlled fields to `user_metadata` makes them user-mutable from any signed-in client (security vulnerability — customer could write their own `subscription_status: active`). ADR-0027 §Risks names the lint/test rule. **`user_metadata` writes are forbidden in ecom code paths.**

**Atomic semantics for auto-claim:** Supabase admin update is not atomically read-modify-write. Auto-claim implementation pattern:
1. Read user via admin API.
2. If `app_metadata.active_machine_id` is null, write the install_uuid via PUT.
3. Race condition: two concurrent first-time sign-ins from the same account → both observe null → both write. Last-write-wins; second machine sees its own value as the active one. Edge case is vanishingly rare (same customer signing in on two machines within the same few hundred ms of first-ever sign-in); accepted per ADR-0027 §4.

---

### 4. JWT structure (for reference; wrapper doesn't expose to Pane)

Pane never sees Supabase JWTs. The wrapper does, internally. Captured here so wrapper code authors know what they're parsing.

**JWT claims** (returned in `access_token` from §1):
```json
{
  "iss": "https://<project-ref>.supabase.co/auth/v1",
  "sub": "550e8400-e29b-41d4-a716-446655440000",
  "aud": "authenticated",
  "exp": 1715095440,
  "iat": 1715091840,
  "email": "matt@example.com",
  "phone": "",
  "app_metadata": { ... },
  "user_metadata": { ... },
  "role": "authenticated",
  "aal": "aal1",
  "amr": [{ "method": "password", "timestamp": 1715091840 }],
  "session_id": "..."
}
```

**Algorithm:** Supabase 2026 default is asymmetric (RS256) with key rotation supported. HS256 still supported for compatibility but deprecated. Wrapper SHOULD use the JWKS endpoint for verification:

`GET https://<project-ref>.supabase.co/auth/v1/.well-known/jwks.json`

**Wrapper does not store or expose Supabase JWTs to Pane.** They are consumed for credential-validation in §1 and discarded.

---

### 5. Out of scope (used by ecom website, not by wrapper)

The following endpoints are used by the ecom website's sign-up flow, not by the wrapper. Captured for context; not used by Pane↔wrapper integration.

- `POST /auth/v1/signup` — sign-up (out of scope per ADR-0027)
- `POST /auth/v1/recover` — password reset (handled at ecom site)
- `POST /auth/v1/verify` — email verification (handled at ecom site)

---

## Auth model used by the wrapper

The wrapper is a **server-side admin client**. It uses two Supabase API keys:

| Key | Usage | Reach |
|---|---|---|
| **anon key** (public-safe, JWT-based per legacy or `sb_publishable_xxx` per new system) | `POST /auth/v1/token` (sign-in credential validation) | Public; safe in wrapper code, but limited authority |
| **service role key** (`sb_secret_xxx` per new system) | `GET /auth/v1/admin/users` (email check), `PUT /auth/v1/admin/users/{id}` (app_metadata writes) | **Bypasses RLS, full project access. Wrapper holds; never logged; never sent to Pane.** |

The new Supabase API key system distinguishes `sb_publishable_` (formerly anon) and `sb_secret_` (formerly service_role) keys per the [Supabase API keys docs](https://supabase.com/docs/guides/getting-started/api-keys). New keys are recommended; legacy JWT-based keys still functional. Wrapper implementation should use the new system at first deployment.

**Service role key handling per ADR-0028:**
- Stored in environment variable / secret manager only.
- Never logged.
- Never sent to client.
- Scope of usage in wrapper: only the operations listed above (admin user lookup, admin user update). No general-purpose service-role usage in wrapper code.

---

## Behavior the wrapper relies on

### `app_metadata` is server-only writable

Documented contract: Supabase RLS prevents user-side writes to `app_metadata`. Wrapper relies on this for the privilege boundary. **If this changes in a future Supabase Auth release, ecom security model breaks.** Capture re-verification cadence catches this; backup mitigation is server-side enforcement in the wrapper itself (refusing reads/writes to `app_metadata` from non-service-role contexts), but Pane never reaches that code path.

### JWT signing key rotation

Wrapper uses Supabase JWTs internally. If Supabase rotates signing keys, JWKS cache (per docs, 10-minute Edge cache) must update. Wrapper SDK or HTTP client should respect cache headers.

### Email confirmation is enforced

`email_confirmed_at: null` results in 400 with `error_description: "Email not confirmed"` on sign-in. Wrapper translates to ADR-0027's `email_not_verified` 403 response.

### Rate limits are platform-level

Supabase Auth enforces internal rate limits (documented but version-pinned). Wrapper's defensive expectations in ADR-0027 §7 are about the wrapper's own limits — Supabase's limits backstop them.

---

## Forbidden patterns (Reed PR review checklist)

For ecom-backend wrapper PRs, Reed's review verifies:

1. **No writes to `user_metadata`** in any ecom code path. All ecom-controlled state lives in `app_metadata`.
2. **No service role key in client-bound code paths.** Service role usage is server-side only; never returned in API responses, never logged.
3. **No pass-through of Supabase JWTs to Pane.** Pane's tokens are wrapper-issued opaque bearers per ADR-0027 §1.
4. **Captured shape match.** JSON deserialization in wrapper code must match the shapes in this document. Drift from this capture is a Reed-blocking finding.

---

## Glossary

| Term | Definition |
|---|---|
| **`app_metadata`** | Supabase user field, service-role-only writable. Holds ecom-controlled state. |
| **`user_metadata`** | Supabase user field, user-writable via own JWT. Forbidden for ecom-controlled state. |
| **service role key** | High-privilege Supabase API key. Wrapper holds; never exposed. |
| **anon key** (or `sb_publishable_`) | Public Supabase API key. Used for sign-in credential validation. |
| **admin API** | Supabase Auth endpoints under `/auth/v1/admin/...`. Service role only. |
| **JWKS** | JSON Web Key Set — Supabase's public-key endpoint for JWT verification. |

## References

- ADR-0027 (contract this capture implements)
- ADR-0028 (platform selection)
- ADR-0023 (capture discipline)
- Reed audit: `docs/audits/2026-05-07-supabase-auth-spec-vs-reality.md`
- Source URLs cited in "Source of truth" section above
