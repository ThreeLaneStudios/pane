# AI Onboarding

**Last Updated:** 2026-11-09 (#3195 / ADR-0070 — local-first launch; account and sync flag-gated)  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §6

> **Cross-references:** See [agents.md](agents.md) §24 for agent entity management, [sync.md](sync.md) §22C for gateway operations.

---

## 6. AI Onboarding
> **Status:** DESIGN LOCKED (#953 / PR #960). NOT BUILT — implementation tracked in sub-tasks #879–#884 and 1–42 (see Dev Dispatch 2026-05-06).
> **Updated:** 2026-05-07 — full rewrite for gateway architecture and account/entitlement model. Old wizard flow (About You / Working Style / Confirm Profile) retired; redesign supersedes.
> **Updated:** 2026-11-09 — ADR-0070 (#3195). The account/entitlement model is removed: Pane opens to Home with no sign-in and no entitlement check. §6.0 (new) describes launch; §6.1 is retitled and flag-gated (`account_enabled`, default off); 6.1.2 and 6.1.3 are retired. The ecom backend (`api.paneapp.ai`) is shut down and has no Pane-side consumer.

Onboarding is the path from a fresh-install Pane to a working Home screen with chat enabled. The customer sees, at most:

1. A provider picker (§6.2), shown over Home on first launch
2. A per-provider flow that configures one of OpenRouter / Anthropic / Venice / Ollama / OpenClaw (§6.3, §6.6)
3. (For OpenClaw) a topology choice and pairing flow
4. Drop into Home, with a welcome message if the provider is verified (§6.5)

There is no sign-in screen on any launch (§6.0, ADR-0070). There is no profile-extraction wizard: agent identity comes from OpenClaw on OC paths, and from per-agent default templates on BYOK paths. There is no celebration screen: completion drops into Home directly.

### State scope

Pane is local-first. There is no account or entitlement state in the launch path (ADR-0070). The account exists only as the identity for encrypted CRDT sync (ADR-0002, ADR-0004, ADR-0014), and the whole account surface is behind the `account_enabled` flag, default off (§6.0). ADRs that bear on this section:

- **ADR-0070** — Local-first launch. Pane opens to Home with no sign-in; account sign-in, session storage and sync stay in the codebase behind `account_enabled`; the migration never deletes the keychain master key or the stored session.
- **ADR-0024** — Local-only decryption. Stands, strengthened: decryption uses OS-keychain credentials only, and no network token is ever required for data access on any launch.
- **ADR-0026** — Install UUID. The `active_machine_id` half is superseded by ADR-0070. `install_uuid` generation and lifetime stand: it is written into backup metadata (§6.1.4).
- **ADR-0025, ADR-0027, ADR-0036** — retired by ADR-0070. **ADR-0029, ADR-0035** — entitlement halves superseded by ADR-0070. Kept in `docs/adr/` with status lines for history.

### Field model

The fields below drive every routing decision in this section. Full state-machine table in Dev Dispatch deliverable 3 (`Pane_Onboarding_Dev_Dispatch__2026-05-06.md`).

| Storage | Field | Type | Drives |
|---|---|---|---|
| Local DB | `onboarding_completed_at` | nullable timestamp | Completion gate; null = onboarding not done |
| Local DB | `onboarding_resume_state` | enum | Per-screen resume on quit-mid-flow |
| Local DB | `provider_type` | enum: `none`, `openrouter`, `anthropic`, `ollama`, `openclaw` | Which provider is configured |
| Local DB | `provider_state` | enum: `none`, `skipped`, `configured_unverified`, `configured_verified` | Banner visibility, retry behavior |
| Local DB | `last_validation_at` / `last_validation_result` | timestamp / enum | Inline note in Settings |
| Local DB (`ai_settings`) | `account_enabled` | `'true'` or absent | Whether the account/sync surface is reachable at all. Absent, any other value, or a failed read = off. No UI writes it. See §6.0. |
| Local config (per ADR-0026) | `install_uuid` | UUIDv4 | Backup metadata (§6.1.4). No longer sent anywhere. |
| OS keychain (per ADR-0024) | provider keys, `gateway_token`, `tls_cert_pin`, `supabase_access_token`, `supabase_refresh_token`, `sync_master_key` | strings | Authenticated provider calls; Supabase bearer and sync master key (flag-gated, §6.1 / §6.1.5). Present on existing installs regardless of the flag. |
| Local config — session metadata file | `user_id`, `email`, `expires_at`, `device_id` | JSON object | Token-expiry gating, account identification, sync identity (flag-gated). See §6.1.5. |

### 6.0 Launch (local-first)
> **Status:** SPEC LOCKED 2026-11-09 (ADR-0070, #3195). Build scope follows.

Every launch opens to Home — Home chat plus the Jobs panel — with no sign-in and no network call in the launch path. On first launch, when `onboarding_completed` is unset, the provider picker (§6.2) is shown over Home; it is the only screen between launch and chat. There is no `AuthScreen`, no S7, no S8, no entitlement check at launch or on focus, and no grace-period toast. `auth_check_entitlement`, `auth_take_over_machine` and `auth_email_check` do not exist.

**`account_enabled` flag.** The account surface (§6.1 sign-in, §6.1.5 session storage, Supabase CRDT sync, Settings → Account) is compiled, tested in CI, and reachable only when the `ai_settings` row `account_enabled` is `'true'`. Absent, any other value, or a failed read is off. Nothing in the UI writes the row. One reader per side:

- Frontend: `initAuth` (`src/lib/stores/authStore.ts`) reads the flag before anything else. Off → `authSession` is `null`, `initAuth` returns immediately — no `initAuthInternal`, no `get_auth_session`, no `cleanup_pending.json` read, no 5 s timeout race. Settings renders its Account & Sync tab — tab button and panel both — only when on. `startAutoSync` is reachable only from `initAuthInternal` / `signIn` and so never runs when off.
- Backend: one `require_account_enabled()` guard at the top of every `sync/commands.rs` command that touches session storage, the master key, or Supabase. Off → `Err("account disabled")`. `get_auth_session` and `is_encryption_ready` are read-only and stay callable so tests can assert state.

**Master-key safety rule (ADR-0070 Decision 5).** The flag-off path never calls `clear_encryption` or `clear_auth_session`, never reads or replays `cleanup_pending.json`, and never runs the token-expiry/refresh branch of `initAuthInternal`. After upgrading an existing install to a flag-off build: keychain `sync_master_key`, `supabase_access_token` and `supabase_refresh_token` present; `~/Pane/.pane/auth_session.json` untouched; `cleanup_pending.json` (if any) left in place; `install-uuid.json` untouched; local DB untouched. Turning the flag on later runs `initAuthInternal` exactly as §6.1 describes, cleanup replay and expiry/refresh included.

**Test-mode bypass removed.** `initAuth`'s `import.meta.env.DEV && window.__PANE_TEST_MODE__` mock session existed to satisfy the startup gate; with the gate gone it is deleted, and the e2e fixtures stop setting the flag. The harness boots to Home because that is what the app does.

### 6.1 Account sign-in (flag-gated, default off)
> **Status:** BUILT (sign-in, session storage, sync) but **not a shipping feature**. Reachable only with `account_enabled = 'true'` (§6.0). Kept for planned team sharing and the phone app. ADR-0070.
> **Updated:** 2026-11-09 — sign-in targets Supabase GoTrue directly; the ecom wrapper (`api.paneapp.ai`, ADR-0027) is retired. 6.1.2 and 6.1.3 are retired stubs.

When the flag is on and no session is stored, Settings → Account offers sign-in. There is no sign-in screen at launch and no in-app signup (`api.authSignUp` invokes an unregistered `auth_sign_up`; it is dead code pending Reed's scope).

#### 6.1.1 Sign-in (Supabase GoTrue)

The Account section presents email and password. On Sign In, `auth_sign_in` POSTs `{email, password}` to `{SUPABASE_URL}/auth/v1/token?grant_type=password` with the project `apikey` header — the same endpoint family and response shape `auth_refresh` already consumes. No email-existence pre-check; no wrapper hop; `install_uuid` and `machine_name` are not sent.

- **Success** → Pane receives GoTrue's token response (`access_token`, `refresh_token`, `expires_at`, `user.id`, `user.email`) and persists it per §6.1.5. No entitlement fields exist and none are consumed.
- **Wrong password / unknown email** → GoTrue returns 400 (`invalid_grant` / `invalid_credentials`, version-dependent; Pane keys on the status, not the code); inline error "Email or password is incorrect." with `[Forgot password →]`, which calls `auth_request_password_reset` (already GoTrue-direct).
- **Rate-limited** → GoTrue returns 429; inline error "Too many sign-in attempts. Try again shortly.", Sign In disabled for 30 s. GoTrue does not return a retry-after countdown; Pane uses a fixed interval.
- **Unreachable** → inline error "Couldn't reach Pane. Check your connection. [Retry]". The app is already open on local data; nothing is blocked.

**Token storage.** `access_token` and `refresh_token` go to the OS keychain; `user_id`, `email`, `expires_at` go to the session metadata file. Full contract in §6.1.5.

**Expiry.** On launch with the flag on, `initAuthInternal` compares `expires_at` to `now()`. Expired → `auth_refresh` (`/auth/v1/token?grant_type=refresh_token`); on success the rotated session is saved and sync is configured; on failure `clear_auth_session` runs and the Account section shows signed-out. (This is what `authStore.ts` has done since PR #1033; the earlier "v1 does not invoke refresh" text was stale against the code and is scrubbed here.)

> **Retired with the wrapper (ADR-0070):** the S1a email-check → S1b password → S1c "no account found" split, `POST /v1/auth/email-check`, `POST /v1/auth/signin`, the entitlement snapshot on the sign-in response, `machine_name` / `active_machine_name`, and the "ecom unreachable hard-blocks first sign-in" rule (X6 case 1). The ADR-0029 privacy posture — no account-derived metadata on an anonymous endpoint — stands as a principle for any future account surface.

#### 6.1.2 Subscription state on entitlement responses — RETIRED
> **Status:** RETIRED 2026-11-09 (ADR-0070, #3195). Previously BUILT v0.65.18 (#973, PR #1060).

Pane no longer consumes `subscription_status`, `subscription_action_url`, `next_payment_attempt`, `amount_due` or `currency`. S7 (lapse hard-block) and the 6.1.2.1 grace-period toast are removed from the code. The ecom wrapper that produced these fields is shut down. ADR-0029 §2 and ADR-0036 are superseded; this stub keeps the section number stable for inbound references. Body as of 1.0.84 is in git history at `2e5109d7`.

#### 6.1.3 Single-active-machine check (take-over) — RETIRED
> **Status:** RETIRED 2026-11-09 (ADR-0070, #3195). Previously BUILT v0.65.18 (#973, PR #1060).

Pane no longer evaluates `active_machine_id`, sends `install_uuid` on sign-in, or calls `POST /v1/account/active-machine`. S8 (`S8TakeOver.svelte`), silent sign-out on cached-token mismatch, and `auth_take_over_machine` are removed. ADR-0025, ADR-0035 and the entitlement half of ADR-0026 are superseded. One piece of this section survives and moves: the **sign-out cleanup retry on next launch** (`cleanup_pending.json`, #1079) is still how a partially failed `signOut()` is retried when the flag is on — its contract now lives in §6.1.5 under `clear_auth_session()`. Body as of 1.0.84 is in git history at `2e5109d7`.

#### 6.1.4 Backup restore offer (X8 case 2)
> **Status:** PARTIALLY BUILT (v0.18.0, issue #972) — backup format extended with install UUID, keychain export, and metadata. UI prompt and restore flow NOT YET BUILT.

Before the picker, Pane checks: is this a fresh-install of the app on a machine that previously had a different `install_uuid`? If yes (which only happens on a new install on a new or reset machine), Pane offers:

> "We've detected this is a new install. Do you have a Pane backup to restore?"
> [Restore from file →] | [Start fresh →]

- **Restore** → file picker. Selected backup is unpacked into local DB + settings + `.pane/system/`. Skips the picker entirely; lands directly in Home with restored data. Backup format includes a plaintext keychain export (`_keychain.json`) and the install UUID (`_install-uuid.json`) per ADR-0024 Amendment 2026-05-07. Sub-task #5 / PR #1006 (#972) implemented this. Backup file confidentiality at rest is the user's responsibility per the amendment's trust boundary.
  - **BUILT (v0.18.0, #972):** Backup format now includes:
    - Install UUID (from `~/Library/Application Support/Pane/install-uuid.json`)
    - Keychain items (account passwords exported to `_keychain.json`)
    - Backup metadata (`_metadata.json`: app version, timestamp, type, email, UUID, schema version)
    - Round-trip restore verified via integration tests
  - **NOT BUILT:** UI prompt, file picker integration, onboarding flow integration
- **Start fresh** → continues to §6.2.

If the customer has no prior install on a different UUID (fresh install on a never-before-used Pane account, or post-wipe reinstall on the same machine where the UUID is regenerated), this screen is skipped.

##### 6.1.4.1 Restore error contract

> **Status:** SPEC LOCKED 2026-05-07. NOT BUILT — sub-tasks #1006 (backend
> error-handling fix) and #971 (restore UI) consume this contract.

The restore flow has fatal errors and non-fatal warnings, distinguished
by whether the failure is recoverable from the user's side at restore
time.

**Step ordering (load-bearing):**

1. Read backup metadata (`_metadata.json`).
2. Schema check: `backup.schema > current` is fatal (block); `backup.schema < current` requires confirmation; `backup.schema == current` proceeds silently.
3. Restore DB.
4. Restore install UUID — fatal on failure (`backup.rs` writes it into backup metadata; a restored install that regenerates its UUID no longer matches the metadata of its own backups).
5. Restore `.pane/system/` — soft on failure (re-syncs from gateway).
6. Restore keychain — soft on failure (recoverable via Settings cascade-UX).

The schema check must precede any state mutation. Steps 4-6 mutate state
only after schema compatibility is established.

**Fatal errors (block restore, return `Err`):**

- `SchemaTooNew` — backup was created with a newer Pane version. UI shows upgrade-Pane message; no "Restore anyway" path.
- `DbRestoreFailed` — SQLite restore failed. UI shows error; user retries or picks a different backup.
- `InstallUuidRestoreFailed` — UUID write failed. UI shows error; user retries before launching Pane (launching with a regenerated UUID makes the install stop matching the metadata of its own backups).

**Non-fatal warnings (continue restore, surface in summary):**

- `KeychainRestorePartial` / `KeychainRestoreFailed` — keychain items
  missing. User re-enters via Settings cascade-UX (§22C.11A.3).
- `SystemFilesRestoreFailed` — `.pane/system/` extraction failed. Files
  re-sync from gateway on next OC connection.

**Schema-older path:** When `backup.schema < current`, restore is paused
for user confirmation before any state mutation. User-cancellation at
this prompt is not an error class; it returns the UI to its pre-restore
state.

**Schema version source (load-bearing).** The `backup.schema` and
`current` values compared in step 2 MUST be read from the schema version
recorded in the `ai_settings` table — the source the migration system
uses — on **both** sides: the version written into a backup's
`_metadata.json` at backup time, and the current version read at restore
time. `PRAGMA user_version` MUST NOT be used: it is always `0` under
cr-sqlite, which collapses this check into a silent no-op (`0 < 0` reads as
“schemas match”). See `backup.md` → “Restore & backup integrity contract”
R4. Pre-launch, backups already written with `schema=0` metadata are nuked,
not migrated.

Full spec including error-class enumeration and UX requirements: see
`docs/spec-prep/2026-05-07-pr1006-spec-decisions/`. The restore UI
(sub-task #971) consumes this contract.

#### 6.1.5 Session storage layer

> **Status:** BUILT (PR #1033). Codifies the storage contract that
> `src-tauri/src/sync/commands.rs` implements; reviewed against ADR-0024
> trust model.

Session state is split across two storage layers per ADR-0024's keychain
stance for sensitive material. The OS keychain holds the bearer
credential (`access_token`) and `refresh_token`; a metadata-only JSON file holds the rest.
This split lets Settings → Account and the sync scheduler read identity
and expiry without unlocking the keychain on every render — only the
bearer is keychain-protected, while the metadata (expiry, identity) is
on disk. Flag-gated: every entry point below returns `Err("account
disabled")` when `account_enabled` is off, except the read-only
`get_auth_session` (§6.0). Storage is never cleared by the flag-off
path (ADR-0070 Decision 5).

##### Storage locations

| Layer | Location | Contents |
|---|---|---|
| OS keychain | service `com.threelane.pane`, account labels `supabase_access_token` and `supabase_refresh_token` | `access_token` (string), `refresh_token` (string) |
| Metadata file | `~/Pane/.pane/auth_session.json`, mode 0600 | `user_id`, `email`, `expires_at`. (`device_id` dropped — machine identity is `install_uuid` per ADR-0026) |

The metadata file is JSON-serialized and atomically written (tempfile +
rename, per file-system.md §2.3A). The keychain entries use the same
service identifier as `gateway_token` and `tls_cert_pin`; the account
label distinguishes per-credential entries.

`refresh_token` is used: on launch with the flag on, an expired
`access_token` is refreshed via GoTrue (`auth_refresh`) and the rotated
session saved; on refresh failure `clear_auth_session` runs and the
Account section shows signed-out (§6.1.1 "Expiry"). The old "v1 does
not invoke it" text was stale against `authStore.ts` and is scrubbed.


##### Entry-point contracts

These four functions in `src-tauri/src/sync/commands.rs` are the only
surface that touches session storage. Any read or write of session state
goes through one of them.

###### `save_auth_session(session)`

Called after a successful `POST /auth/v1/token?grant_type=password` response (§6.1.1) and
after a successful password change (`auth_change_password` contract
below). Writes the session payload across both layers:

- Writes `access_token` and `refresh_token` to keychain.
- Writes `{user_id, email, expires_at, device_id}` to
  `~/Pane/.pane/auth_session.json` (atomic).

Either-failure mode: if the keychain write fails after the JSON write,
or vice versa, `save_auth_session` returns an error and the caller
surfaces a hard sign-in error per X6 case 1. Partial state (one layer
written, one not) is detectable on the next `get_auth_session` call
and treated as no-session.

The GoTrue token response carries no entitlement fields; everything in
it that Pane keeps is listed above. (The former entitlement snapshot —
`subscription_status`, `active_machine_id` and friends — is retired per
ADR-0070.)

###### `get_auth_session() -> Option<AuthSession>`

Called at app launch, on-focus, and before any authenticated API call.
Returns the assembled session for use as `Authorization: Bearer`:

- Reads `access_token` and `refresh_token` from keychain.
- Reads metadata fields from `~/Pane/.pane/auth_session.json`.
- Returns `Some(AuthSession { access_token, refresh_token, user_id,
  email, expires_at, device_id })` only if both reads succeed.

Failure modes returning `None` (= no session; Settings → Account shows signed-out):

- Keychain has no `access_token` entry.
- Metadata file does not exist or fails to parse.
- Metadata file exists but keychain is missing the entry (orphaned
  metadata from a partial `clear_auth_session` failure or migration
  edge case).
- Keychain has an entry but metadata file is missing.

Expiry is **not** checked by `get_auth_session`; it returns the session
regardless of `expires_at`. The caller (`initAuthInternal`, flag on)
checks `session.expires_at > now()` itself and refreshes via
`auth_refresh` if expired (§6.1.1 "Expiry"); on refresh failure it
calls `clear_auth_session`.

###### `clear_auth_session()`

Called on (flag on only — the flag-off path never calls it, ADR-0070
Decision 5):

- Sign-out from Settings → Account (`signOut()` in `authStore.ts`),
  after `clear_encryption`.
- Refresh failure on launch (§6.1.1 "Expiry").
- Any state-recovery path that needs a clean no-session state.

**Sign-out cleanup retry (moved here from retired §6.1.3, #1079).** When
`signOut()` partially fails — `clear_encryption` (master key deletion)
or `clear_auth_session` throws — the app writes
`~/Library/Application Support/Pane/cleanup_pending.json`
(`{ attempts, created_at, cleanup_encryption, cleanup_auth_session }`).
On the next launch **with the flag on**, `initAuthInternal` retries only
the halves marked `true`, increments `attempts`, deletes the flag on
success or after 3 attempts (logged at ERROR, stale state accepted), and
deletes it on any successful sign-in. With the flag off the file is
neither read nor touched.

Removes the session from both layers:

- Deletes the `access_token` and `refresh_token` keychain entries.
- Deletes `~/Pane/.pane/auth_session.json`.

Either-half failure is logged but not fatal; subsequent
`get_auth_session` calls treat partial state as no-session. The function
returns success if either half succeeds (the no-session post-condition
is met).

This function does **not** call `POST /v1/auth/signout` — that is a
separate concern handled by the sign-out caller (Settings) when it
wants server-side invalidation in addition to local clearing.

###### `auth_change_password(new_password: String) -> Result<AuthSession, String>`

Called from the Settings password-change UI. Sends `PUT /auth/v1/user` to Supabase with the new password, persists the rotated tokens Supabase returns, and reports success/failure to the caller.

Behavior:

- Reads the current session via `get_auth_session` to populate the `Authorization: Bearer <access_token>` header for the outgoing request and to retain the identity fields needed to construct the new session.
- On HTTP 200, parses the response body for the rotated `access_token` and `refresh_token` Supabase issues as a side effect of the password change.
- Constructs the new `AuthSession` by combining the rotated tokens with the existing identity fields preserved from the prior session (`user_id`, `email`, `expires_at`, `device_id`) — these fields do not change on password rotation.
- Calls `save_auth_session` with the new `AuthSession` to update both storage layers (keychain entry for `access_token`, metadata file for `refresh_token` plus the preserved identity fields) per `save_auth_session`'s atomic-write contract.
- On success, returns `Ok(new_session)` to the caller.
- On failure (HTTP error, response-parse error, or persistence error from `save_auth_session`), returns `Err(message)`. Existing session is left intact in storage; partial-state recovery flows through `get_auth_session`'s existing orphaned-state detection, which routes the customer to S1. The customer's recovery path is signing in again with the new password just set.

Storage side effects: writes to both layers via `save_auth_session` — keychain entry updated with the new `access_token`, metadata file rewritten with the new `refresh_token` plus the preserved identity fields. No other surface (`Settings.svelte`, etc.) needs to invoke `save_auth_session` separately; the persistence is fully encapsulated inside this function.

Caller-side: `Settings.svelte` shows "Password changed successfully" on `Ok` return and surfaces the error message on `Err` return; it does not need to manage token persistence.

> **Status:** Describes the post-#1036-fix behavior. Ships with #1036's PR. Prior to #1036, `auth_change_password` returned `Result<(), String>`, discarded the rotated tokens from the response body, and did not call `save_auth_session`; the keychain retained the server-invalidated `access_token` and the next authenticated call failed silently. The fix updates both the signature and the persistence behavior, encapsulating session rotation inside the function so callers don't need to be aware of token rotation.

##### Migration from pre-PR-#1033 plaintext format

Customers who installed Pane before PR #1033 have an
`~/Pane/.pane/auth_session.json` file containing the full session
including `access_token`. On first launch under any post-#1033 version,
Pane detects the legacy format (presence of `access_token` in the JSON)
and migrates:

1. Read the legacy JSON.
2. Extract `access_token` and `refresh_token`, write both to keychain.
3. Re-write the JSON without token fields (metadata-only format).

Migration is best-effort: if either step fails, the legacy file is left
in place, `get_auth_session` returns `None` (no keychain entry), and
the customer signs in again from Settings → Account. Existing session is
invalidated; acceptable for a one-time migration step. Runs only when
the flag is on (it lives in `get_auth_session`, which `initAuth` does
not call when off).

This migration logic lives in `get_auth_session` and runs
idempotently — once the JSON is in metadata-only form, the migration
check is a no-op.

### 6.2 Provider picker (screen 2)
> **Status:** NOT BUILT (sub-task #38 shared BYOK picker component; provider routing in #879–#884)
> **Note:** Legacy `ModelPicker.svelte` removed (dead code — never rendered; `proceedFromModelPicker()` removed in #882). All model selection now uses `SearchableModelSelect`.

On first launch (or backup-decline), the customer sees the provider picker over Home (§6.0). Cards stacked full-width (wrapping to a second row as the provider count grows) — same shape as the existing provider-selection screen — labeled only with provider names.

> **#2455 venice live-test v2 ruling (2026-08-30):** the onboarding picker shows every configured/addable provider dynamically, not a hardcoded subset — Venice was added to the card list as the first case of this ruling, and any future runtime must be added here too. See `Onboarding.svelte`'s provider-enumeration entry-point census comment (cross-referencing `NewProviderModal.svelte`, `DefaultAgentPickerModal.svelte`, `ProviderConnectionsPanel.svelte` — the other three independent per-runtime enumeration sites in this codebase) and `docs/audits/2026-08-30-venice-livetest-v2-scope.md`.

```
Connect Your AI

[OpenRouter]
[Anthropic]
[Ollama]
[OpenClaw]

New to AI providers? [Help →]

Skip — set this up later
```

Card click routes per the table below. There is no pre-selected default — the existing pre-highlight on OpenRouter is wrong because Ollama and OpenClaw don't take API keys (decision in dev dispatch X1).

| Card | Click action |
|---|---|
| OpenRouter | §6.3.1 (BYOK key entry) |
| Anthropic | §6.3.2 (BYOK key entry) |
| Venice | BYOK key entry, same shared shape as OpenRouter/Anthropic (`validate_venice_key`) — added per the #2455 dynamic-picker ruling above |
| Ollama | §6.3.3 (silent localhost probe; routes to model picker, "no models" state, or custom-endpoint screen depending on result) |
| OpenClaw | §6.6.3 (SplitMachine — URL + token entry) |
| Skip | Drop into Home with the persistent banner ("Connect AI to enable chat → Settings"). `provider_state = skipped`. See §6.5.1. |

Skip is available **only** on this screen. Subsequent screens have Back, not Skip — different intents, different controls (per dev dispatch X2).

Card content is provider name plus a short one-line descriptor (`.provider-desc`) — see the #2792 amendment below for the current descriptor text and the fixed-height card contract shared with §6.4.1. A customer who doesn't recognize a provider still has the bottom-of-screen help link for anything the one-liner doesn't cover.

#### 6.2.1 Back behavior across paths

Every post-picker screen has Back. Back from the first sub-step of any path returns to the picker; back within a multi-step path goes to the previous sub-step. The picker is the hub; every path is a spoke. No state cached across path switches — pasting a key takes 30 seconds and a cached-key state model would leak.

The picker on return looks identical to first visit. No "you tried OR, here's why it failed" messaging on return. Failure messaging happens on the path screen, not on return to picker (X3).

### 6.3 Per-provider flows — BYOK paths
> **Status:** NOT BUILT (sub-tasks #26–#39, plus shared component #38)

OR, Anthropic, and Ollama all use a shared key-entry → validate → model-picker pattern with provider-specific copy and validation endpoints. Component (#38) is shared; per-provider config differs.

External-system contracts for these endpoints are captured in the spec-prep external-contract-captures document (`docs/spec-prep/2026-05-07-external-contract-captures.md`). Per ADR-0023, code adapts those shapes; the spec captures are canonical reference.

#### Visual contract — shared connect-screen error UI

The `.connect-error-msg` class shared across onboarding connect screens MUST include `margin-bottom` (≥8px) so error text does not crowd the primary action button (Continue / Connect) below it. Applied uniformly via the shared class; per-site overrides forbidden.

#### 6.3.1 OpenRouter

**Step 1 — API key entry.** Single screen, single field, `Test Connection` button.

```
Connect OpenRouter
Paste your OpenRouter API key.

[ sk-or-v1-...                                                  ]
Don't have one? [Get a key →]   (links openrouter.ai/keys)

[Test Connection]                                          [Back]
```

No format check on blur — single validation surface at Test Connection time, format and auth combined in one error path (per X5). Interrupting paste → Test-Connection with blur errors creates friction with no benefit.

**Step 2 — Test Connection: `GET /api/v1/auth/key`.** Capture: external-contract-captures §4. Returns key validity + credit balance in a single call; no credits consumed. Outcomes:

| Wire result | UX |
|---|---|
| 200 with `data.limit_remaining > 0` | Advance to Step 3 |
| 200 with `data.limit_remaining = 0` | "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]" (eventual-consistency hint per X5) |
| 401 | "Key rejected. [Get a new key →]" |
| 402 | Account suspended or credits cannot cover the request — a billing state, NOT a bad key. Surface the no-credits + propagation copy (same as `limit_remaining = 0`), never "Key rejected." [Add credits →] (`openrouter.ai/settings/credits`) |
| Network error | "Couldn't reach OpenRouter. Check your connection. [Retry] [Skip — set up in Settings]" — Skip writes `provider_state = configured_unverified` per X6 case 2 |
| 429 (rare) | "Too many attempts. Wait a moment and try again." |
| 5xx | Treat same as network error |

Format-invalid (key doesn't match `sk-or-v1-...`) is detected from the 401 response or short-circuited locally; surface as: "That doesn't look like a valid OpenRouter key. They start with sk-or-v1-."

> **Path note:** Three different path strings exist in the wild. Shipped code uses `/api/v1/auth/key` (`src-tauri/src/system_files.rs:628`); vendor docs publish `/api/v1/key`; dev dispatch shorthand was `/auth/key`. Trust order is shipped code first; the spec uses the shipped-code path. Vendor-docs alignment (whether OR's `/api/v1/auth/key` is a stable alias or a deprecated duplicate of `/api/v1/key`) is a separate side issue; not in scope for the redesign spec-lock.

**Validator contract (`validate_openrouter_key`).** The validator MUST distinguish three outcomes and never collapse them:

- **401** → bad/revoked key → "Key rejected."
- **402** → account suspended or credits cannot cover the request → a billing state, not a bad key. Surface the no-credits + propagation copy (as for `limit_remaining = 0`); never "Key rejected." See ai-integration.md §4.3.2.
- **200** → the validator MUST inspect `data.limit_remaining`. `limit_remaining = 0` surfaces the no-credits + propagation hint; a 200 is NOT treated as success without this check. A valid key with zero credits MUST surface the no-credits copy, not silently advance to Step 3.

**Step 3 — Model picker.** On Test Connection success, fetch the model list from OR's `/api/v1/models` (capture pending — see external-contract-captures §4.4) and present in the shared picker (#38). Search box, full list at ≤20–25 items, prominent search above that threshold. Continue is disabled until a model is selected; no auto-select default. On model-fetch failure: "[Retry] [← Back]" — Back navigates to the provider connect screen (`connect`) and clears `primaryModelError`.

> **#2617 fix note:** this step's model fetch (`fetchPrimaryModels()` in `Onboarding.svelte`) resolves the provider via credential-based derivation — `fetch_available_models` / `connected_runtime_providers` — not the agent-based `derive_active_provider_label` (which requires an existing `ai_agents` row and is correct only for its other callers, e.g. Settings/status). This step runs before any agent exists, so agent-based derivation always failed with "No primary provider configured" on a fresh install; the merged, provider-namespaced response is filtered client-side to `connectProvider`, mirroring `AgentCreateOverlay.svelte`'s `loadModelsForRuntime()`.
>
> **Empty-after-filter has two distinct causes, and the copy must distinguish them.** `fetch_available_models` (`ai::connections`) only returns `Err` when *every* connected runtime's fetch fails (`all_models.is_empty() && !errors.is_empty()`) — a partial failure where `connectProvider`'s own fetch failed silently but at least one *other* connected runtime succeeded still resolves `Ok`, with `connectProvider` simply missing from the merged response entirely. That is a provider-unreachable failure and looks nothing like "connected, but this account/instance genuinely has zero models available" (e.g. a real, reachable-but-empty Ollama daemon) — conflating the two left users unable to tell whether to retry a network issue or stop expecting models to appear. Checking whether `connectProvider` appears anywhere in the *unfiltered* merged list cannot distinguish these cases — that check is logically identical to the filter that just produced zero results (if any entry for `connectProvider` existed in the merged list, it would also have survived the filter), so it's always false here regardless of which case actually occurred. `fetchPrimaryModels()` instead re-probes `connectProvider`'s live reachability via the same commands `testConnection()`/`probeOllama()` already called moments earlier with the same credentials (`validate_anthropic_key`/`validate_openrouter_key`/`ollama_probe_two_step`, wrapped in a new `probeConnectProviderReachable()` helper) and selects copy from that: reachable now → `No models available for <Provider>.`; unreachable → `Could not reach <Provider>. Check your connection and try again.`

**Step 4 — Continue.** Persists `provider_type = openrouter`, `provider_state = configured_verified`, `last_validation_at = now()`, encrypted `api_key` to keychain, selected `model_id` and `last_models_fetch_at` to DB. Advances to §6.4.

#### 6.3.2 Anthropic

Same pattern as §6.3.1 with provider-specific config:

| Field | Value |
|---|---|
| Title | "Connect Anthropic" |
| Placeholder | `sk-ant-...` |
| Get-a-key link | console.anthropic.com/settings/keys |
| Validation endpoint | `GET /v1/models` (capture: external-contract-captures §5) |
| Auth header | `x-api-key: <key>` (note: not Bearer) plus `anthropic-version: 2023-06-01` |
| Validates credits | **No** — `/v1/models` does not probe billing |

Outcomes table is identical to OR except:

- "No credits" path is **not present** at onboarding. Anthropic's `/v1/models` succeeds on a paid-but-zero-balance key. The customer sees the credit issue only on first chat, not at onboard. This is documented limitation per X5.
- Anthropic error envelope (capture §6) uses `{"type":"error","error":{"type":"<error_type>","message":"..."},"request_id":"..."}` shape. Adapter parses `error.error.type` for classification.
- Anthropic billing-error misclassification quirk (capture §6): a 400 `invalid_request_error` with `"credit balance"` substring in the message must be treated as billing — sub-task #1 covers this in the shared error handler (bites every Anthropic call, not just onboarding).

Step 3 model picker fetches `/v1/models` and uses the shared component. Stores model `id` (e.g., `"claude-sonnet-4-20250514"`) for chat calls; uses `display_name` for the picker UI label.

#### 6.3.3 Ollama

Local probe path. No auth surface, no key-entry screen.

**Step 1 — Silent localhost probe.** On picker click, Pane probes `GET http://localhost:11434/api/tags` (capture: external-contract-captures §7) without showing a transition screen.

| Probe result | Routes to |
|---|---|
| 200, `models` non-empty | Step 2 model picker |
| 200, `models: []` | "No models installed" empty state (sub-task #36): Pane-hosted help with the `ollama pull <name>` command and a link to ollama.com/library |
| Connection refused (Ollama not running, or not installed) | Custom-endpoint screen (Step 3) plus "[Help with Ollama →]" link to install help (sub-task #37) |
| `/api/version` 200, `/api/tags` non-200 or unparseable (`tags_failed`) | Custom-endpoint screen (Step 3), copy: "Found Ollama but couldn't list models — check endpoint or retry" |

The IPv6-vs-IPv4 dual-stack pattern observed in Reed's capture (IPv6 refused, IPv4 connected) is normal and not a probe failure — the probe only fails if both fail.

**Step 2 — Model picker.** Same shared component as OR/Anthropic. Display list strips `:latest` suffix, shows non-latest tags verbatim, flat alphabetical (sub-task #39). Internal `model` field preserved for API calls.

**Step 3 — Custom endpoint (escape valve).** When localhost probe fails or the customer wants a non-default endpoint:

```
Couldn't find Ollama at localhost:11434.

Endpoint URL
[ http://localhost:11434                                         ]

[Try again]   [Custom endpoint]   [Help with Ollama]   [Back]
```

URL field auto-prepends `http://` and `:11434` if missing (sub-task #34). On Continue, probe the custom endpoint; on success, route to Step 2.

**Step 4 — Continue.** Persists `provider_type = ollama`, `endpoint_url`, `model_id`, `last_tags_fetch_at` to DB. Ollama has no API key, so nothing lands in keychain. Advances to §6.4.

### 6.3A Onboarding Check Logic
> **Status:** BUILT v0.19.1 (#398). PRESERVED VERBATIM from prior §6.3A — verdict SURVIVES per Reed's audit 2026-05-07.

**Implementation:** `should_show_onboarding()` in `notes.rs` is the single source of truth for whether to show onboarding. The frontend calls this function instead of checking `ai_settings.onboarding_completed` directly.

**Check order:**
1. If `onboarding_completed` = `"true"` → skip onboarding (fast path)
2. If `onboarding_completed` = `"false"` → show onboarding (explicit user intent to re-onboard)
3. If flag missing, check for existing content (notes, projects, chat messages):
   - **Notes exist** (non-deleted) → auto-set flag to `true`, skip onboarding
   - **Projects exist** (non-system) → auto-set flag to `true`, skip onboarding
   - **Chat messages exist** → auto-set flag to `true`, skip onboarding
4. If database is completely empty → show onboarding (fresh install)

**Self-healing:** When content exists but the `onboarding_completed` flag is missing (due to migration bugs, DB wipes with content preserved, or manual DB edits), the function auto-sets the flag to `true`. Next launch hits the fast path. This prevents onboarding from re-triggering on upgrade for users with existing content.

**Why not just check the flag?** Previous implementations had a migration bug that lost the `onboarding_completed` flag while preserving user content (notes, projects, chat history). Checking only the flag would force these users through onboarding again, losing gateway credentials and agent configuration. The content-based fallback recovers gracefully.

**Frontend integration:** `+page.svelte` calls `invoke('should_show_onboarding')` instead of `get_ai_setting('onboarding_completed')`. Database errors (lock contention, corruption) return `false` and do NOT trigger onboarding — they trigger an error alert instead.

> **Bridge to redesign field model:** the `onboarding_completed` flag continues to be the boolean gate this function returns. The redesign's `onboarding_completed_at` timestamp (Dev Dispatch deliverable 3) is the timestamp variant — set at the §6.4 finish line (Submit on a sub-flow or Skip at the §6.4.1 picker) per §6.5.2. Both fields exist; `should_show_onboarding()` reads either form (truthy timestamp OR `"true"` string flag) for backward compatibility through migration.

### 6.4 Smart Features (optional)
> **Status:** NOT BUILT (umbrella #1199 Smart Features Setup; landing PR scoped under Phase 1)
> **Venice sub-flow (§6.4.1 picker card, §6.4.5 Path A):** BUILT (PR #2795, ADR-0065 Supplement S1)

Smart Features is opt-in onboarding setup after primary provider configuration. Pane uses an internal AI provider — separate from the primary the customer just configured — to power three background AI features: extraction (memory + daily logs), Smart Routing (tier-based model dispatch), and Rerank (memory-search result re-ordering). Each feature has its own model setting; each gates independently at runtime (§6.4.6).

> **Smart Features use their own provider setting, separate from the agent's chat provider; choose Venice there too if you want every AI call on Venice.** (#2988) This applies to every primary provider, not just Venice — picking a primary provider here configures chat only; Smart Features' provider is a second, independent choice made on this screen (or later via Settings → AI → Smart Features per §27.6.2).

Customers can configure Smart Features now, or skip and configure later in Settings. Skipping is allowed on the §6.4.1 picker page. Once the customer commits to a provider sub-flow, there is no in-flow skip — they finish setup or back out to the picker.

When shown:
- After the primary provider reaches `configured_verified` state (§6.3.x for BYOK primary, §6.6.x for OC primary).
- Before §6.5 Welcome.
- Customers with `provider_state ∈ {configured_unverified, skipped}` (primary picker Skip or primary X6 case 2 Skip-to-unverified) **do not reach §6.4** — they land directly in Home per §6.5.1.

#### 6.4.1 Picker screen

```
Connect Smart Features

Pane can use a separate AI provider for memory and daily logs.
Pick one, or skip to set up later.

[<one card per eligible configured provider — ADR-0065 S1 universal rule>]

Skip — set up later

                                                            [Back]
```
The internal provider dropdown offers every configured provider except those excluded by a capability gate (ADR-0065 S1). OpenClaw may serve as fallback per existing behavior. Cards: provider name plus a one-line descriptor, no icons — same shape and same fixed-height card contract as §6.2 picker cards (see the #2792 amendment below). Descriptors mirror the §6.2 wording for the same provider, kept in sync so the two pickers never describe the same provider two different ways. Hover state. Click routes per the table below.

| Control | Click action |
|---|---|
| Provider card | Provider's per-runtime sub-flow (§6.4.2 OpenRouter, §6.4.3 Anthropic, §6.4.4 Ollama; new runtimes exercise the same pattern per ADR-0065 S1) |
| Skip — set up later | §6.4.5 skip path — writes skipped state + completion timestamp, routes to §6.5 Welcome |
| Back | Returns to the previous step's completion screen (§6.3.x or §6.6.x) |

Skip is the only Skip control in §6.4. No equivalent control exists inside §6.4.2 / §6.4.3 / §6.4.4 sub-flows. Network errors inside setup surface a Retry inline; if the customer cannot resolve, they back out to the picker and Skip from there.

No state is cached across path switches within §6.4. Entering a sub-flow then backing out to the picker discards any in-flight credential entry or partially-fetched model list. Re-entering re-fetches.

#### 6.4.2 OpenRouter sub-flow

Two steps. Step 1 is credential entry + validation; Step 2 is the three model pickers.

**Step 1 — API key entry.**

```
Smart Features — OpenRouter

Paste your OpenRouter API key.

[ sk-or-v1-...                                                    ]
Don't have one? [Get a key →]   (links openrouter.ai/keys)

[Test Connection]                                          [Back]
```
Back returns to the §6.4.1 picker. On Test Connection, Pane validates against `GET /api/v1/auth/key` (capture: external-contract-captures §4) — same call as §6.3.1 step 2.

The customer enters a key here regardless of what the primary provider is. If primary is also OpenRouter and the same OR account is appropriate for Smart Features, the customer re-pastes the same key — Pane writes a separate keychain entry. No reuse affordance. Rationale: two keychain entries with identical values is operationally cheap; sharing a pointer between roles complicates provider-switching lifecycle (see Gap 4 in session notes).

Validation outcomes (same shape as §6.3.1):

| Wire result | UX |
|---|---|
| 200 with `data.limit_remaining > 0` | Validation succeeds → Step 2 |
| 200 with `data.limit_remaining = 0` | Inline: "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]" |
| 401 | Inline: "Key rejected. [Get a new key →]" |
| Network error | Inline: "Couldn't reach OpenRouter. Check your connection. [Retry]" — no in-setup Skip. Back-to-picker → Skip if persistent. |
| 429 | Inline: "Too many attempts. Wait a moment and try again." |
| 5xx | Treat same as network error |

Format-invalid detection: identical to §6.3.1 (401 response or short-circuit local check on `sk-or-v1-` prefix).

**Step 2 — Model pickers.**

On Step 1 success, Pane fetches the OR model list from `/api/v1/models` (capture pending — see external-contract-captures §4.4) and displays the three-picker screen. Picker labels and descriptions are **verbatim from Settings → AI → Smart Features** (per ADR-0037 §3 amendment, pending Memory/AI session) — onboarding and Settings read identically:

```
Pick models for each feature

Extraction Model
Model for memory extraction from conversations
[ search models... or pick from list                              ]

Smart Routing Model
Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.
[ search models... or pick from list                              ]

Rerank Model
Improves memory search accuracy
[ search models... or pick from list                              ]

[Submit]                                                    [Back]
```
Each picker uses the shared BYOK model picker component (#38) with adaptive search treatment per existing §6.3.1 Step 3 rules. Same model list populates all three pickers; the customer picks independently for each feature. No auto-select default in any picker.

Submit is disabled until **Extraction Model** is picked. This is the Submit-gate behavior — the "extraction is core" framing is carried by the gate, not by picker-label decoration. **Smart Routing Model** and **Rerank Model** are not required for Submit; leaving either picker empty is a valid completion (null = that feature silently disabled at runtime per §6.4.6).

On Submit, state writes per §6.4.5 fire and the flow advances to §6.5 Welcome.

Model list fetch failure on Step 2 load surfaces inline: "Couldn't load OpenRouter models. [Retry]" — auth state is preserved (key still validated from Step 1). Persistent failure recovers via Back → picker → Skip.

Back from Step 2 returns to Step 1 (key entry). Re-entering Step 2 from a successful Step 1 re-fetches the model list. No model-selection state is cached across Back navigation.

#### 6.4.3 Anthropic sub-flow

Same two-step shape as §6.4.2 with provider-specific config.

**Step 1 — API key entry.**

```
Smart Features — Anthropic

Paste your Anthropic API key.

[ sk-ant-...                                                      ]
Don't have one? [Get a key →]   (links console.anthropic.com/settings/keys)

[Test Connection]                                          [Back]
```
Validates against `GET /v1/models` (capture: external-contract-captures §5). Auth header is `x-api-key: <key>` plus `anthropic-version: 2023-06-01` (note: not Bearer — same conditional-header rule as §6.3.2).

Validation outcomes match §6.3.2: no "no credits" case (Anthropic's `/v1/models` doesn't probe billing), 401 → "Key rejected", error envelope per `error.error.type` per capture §6, billing-error misclassification quirk (400 + `invalid_request_error` type + `"credit balance"` substring → treat as billing; shared helper per ai-integration.md §4.3.1) applies here as it does in §6.3.2.

Network error: Retry-only-no-in-setup-Skip per §6.4.2 rules.

**Step 2 — Model pickers.**

Identical layout to §6.4.2 Step 2, including Settings-verbatim labels (`Extraction Model` / `Smart Routing Model` / `Rerank Model`) and descriptions. Anthropic's `/v1/models` response (capture §5) populates the picker; the same response feeds all three pickers. The customer picks independently per feature. Extraction Model required to enable Submit; Smart Routing Model and Rerank Model optional.

#### 6.4.4 Ollama sub-flow

Local probe path. No key concept — endpoint-based. Two steps; Step 1 is endpoint entry + two-call presence probe; Step 2 is the three model pickers.

**Step 1 — Endpoint entry and presence probe.**

```
Smart Features — Ollama

Endpoint
[ http://localhost:11434                                          ]

[Test Connection]                                          [Back]
```
Field defaults to `http://localhost:11434`. Customer can override (remote Ollama, non-default port).

On Test Connection, Pane runs the two-call presence probe:

1. `GET <endpoint>/api/version` — confirms Ollama is reachable (capture: external-contract-captures §8).
2. `GET <endpoint>/api/tags` — fetches the local model list (capture: external-contract-captures §7).

Both must succeed for validation to pass. Rationale (full): see external-contract-captures §8 "Why two calls instead of one."

Validation outcomes:

| Wire result | UX |
|---|---|
| Both 200, `/api/tags` returns `models: [...]` non-empty | Validation succeeds → Step 2 |
| `/api/version` 200, `/api/tags` 200 with `models: []` | Inline: "Ollama is running but no models are installed. Run `ollama pull <model>` in Terminal — for example `ollama pull llama3.2:3b` — then retry. [Browse models →]" (`ollama.com/library`) |
| `/api/version` connection refused | Inline: "Couldn't find Ollama at `<endpoint>`. Make sure Ollama is running. [Try again] [Help with Ollama →]" |
| `/api/version` 200, `/api/tags` non-200 | Inline: "Couldn't list models. Try again or check the Ollama logs." |
| Any 5xx | Treat same as connection refused |

No in-setup Skip on any of these — Retry only. Back-to-picker → Skip remains the recovery path for persistent failure.

`:latest` tag stripping for display per §6.3.3 / sub-task #39 still applies.

**Step 2 — Model pickers.**

Identical layout to §6.4.2 Step 2, including Settings-verbatim labels and descriptions. The `/api/tags` response from Step 1 populates the picker (one fetch, three pickers). The customer picks independently per feature. Extraction Model required to enable Submit; Smart Routing Model and Rerank Model optional.

For small Ollama model inventories (typically 1–5 models), the shared picker component's adaptive search treatment renders the full list visible immediately with search collapsed-but-available per §6.3.3 / shared-component #38 rules.

If the customer changes the endpoint after reaching Step 2 — they don't have that affordance; the endpoint field is on Step 1 only. Back navigates to Step 1 to change endpoint; the model list re-fetches on next Step 1 → Step 2 advance.

#### 6.4.5 State writes — finish line

§6.4 is the onboarding finish line. Two paths write the completion timestamp.

**Path A — Submit after sub-flow completion.**

On successful Submit from §6.4.2 / §6.4.3 / §6.4.4 Step 2:

```
internal_provider_type = openrouter | anthropic | ollama | venice
internal_provider_state = configured_verified
internal_provider_last_validation_at = now()
extraction_model = <selected model id>                    // always set on Submit (Submit-gated)
simple_tier_model = <selected model id> | null            // null if customer did not pick
memory_search_llm_rerank_model = <selected model id> | null   // null if customer did not pick
onboarding_completed_at = now()
onboarding_resume_state = complete
```
Plus the keychain write per sub-flow:

- **§6.4.2 (OpenRouter):** new keychain entry `com.threelane.pane.openrouter.smart_features` with the entered key. Always a new row, regardless of whether primary is OR with the same key.
- **§6.4.3 (Anthropic):** new keychain entry `com.threelane.pane.anthropic.smart_features` with the entered key. Always a new row.
- **§6.4.4 (Ollama):** no keychain entry (Ollama is endpoint-only). `internal_provider_endpoint` stored in `ai_settings`.
- **Venice sub-flow (#2795):** no separate `.smart_features`-suffixed entry. Venice is keychain-only with exactly one dedicated account (`ai_venice_key`, ADR-0065 D4) shared by primary and Smart Features roles alike — there is no per-role Venice key the way OpenRouter/Anthropic have two rows. Submitting a Venice key here (re)writes the same `ai_venice_key` account a primary-Venice connection would have written; there is no dual-entry, no reuse affordance, and no plaintext fallback (ADR-0065 D4).

The keychain naming convention mirrors the primary-provider pattern from §6.1.5 storage layer: `com.threelane.pane.<provider>` for primary, `com.threelane.pane.<provider>.smart_features` for the internal-provider role, for every provider **except Venice**, which is single-account per D4 (see bullet above) — new runtimes follow whichever precedent matches their own primary-path storage shape, not a blanket dual-entry rule.

This skip/Submit distinction, and the provider list above, are open-domain per ADR-0065 S1: any configured provider capable of Smart Features dispatch (i.e. not excluded by a capability gate) reaches this finish line through its own sub-flow — the enumeration above is illustrative of providers built to date, not a closed set.

**Path B — Skip on §6.4.1 picker.**

On clicking Skip — set up later:

```
internal_provider_state = skipped
onboarding_completed_at = now()
onboarding_resume_state = complete
```
No `internal_provider_type`, no model fields, no keychain writes. The customer can configure Smart Features post-onboarding via Settings → AI tab → Smart Features section per ADR-0037 §3 (amendment pending — see Memory/AI spec session).

**Both paths advance to §6.5 Welcome** after their state writes complete. §6.5 itself writes no completion state — the writes have already happened at the §6.4 finish line.

#### 6.4.6 Per-feature runtime gate

> **Cross-references:** ADR-0038 (amended 2026-05-19) + smart-routing.md §27.6 — call-path matrix for which dispatcher reads which model setting, behavior on null.

Each Smart Features-driven feature reads its own canonical model setting at runtime:

| Feature | Reads | Behavior when set | Behavior when null |
|---|---|---|---|
| Extraction (memory + daily logs) | `extraction_model` | Runs via internal provider, using selected model | Silently disabled — no extraction, no daily logs from memory pipeline |
| Smart Routing (tier-based model dispatch) | `simple_tier_model` | Runs via internal provider | Silently disabled — chat continues via primary at a single tier |
| Rerank (memory-search result re-ordering) | `memory_search_llm_rerank_model` | Runs via internal provider | Silently disabled — memory search returns un-reranked results |

"Silently disabled" means: the feature does not run, no error surfaces to the customer, no toast or banner appears. In Settings → AI tab → Smart Features section, the corresponding pickers render with their current value (which may be empty) per ADR-0037 §4. **Chat continues to work via the primary provider regardless of any null fields.**

Three paths reach the runtime gate with model fields null:

1. **Customer Skipped at §6.4.1 picker.** All three fields null. All three features disabled.
2. **Customer Submitted §6.4 with Extraction Model picked, Smart Routing Model / Rerank Model unpicked.** Extraction runs; Smart Routing / Rerank disabled.
3. **Existing pre-Smart-Features customer (lazy re-entry case).** Customer past `onboarding_completed_at` from before §6.4 existed. All three fields null because §6.4 never ran. All three features disabled until customer configures Smart Features post-launch via Settings.

No cross-feature fallback. `extraction_model` does not fall back to `simple_tier_model`, primary, or any other setting. Null means disabled; the customer enables a feature by setting its model in Settings → AI tab → Smart Features.

#### 6.4.7 Network errors during setup — no in-setup Skip

§6.4 does not have a `configured_unverified` state for the internal provider, unlike primary providers in §6.3 X6 case 2.

Network errors during validation surface a Retry inline. If the customer cannot resolve:

- They navigate Back to the §6.4.1 picker.
- From the picker, they click Skip — set up later.
- The skip path (§6.4.5 Path B) writes `internal_provider_state = skipped` and completes onboarding.

Rationale for asymmetry with §6.3 X6 case 2: the §6.3 unverified-skip path exists because the customer has *committed* to a primary provider (chosen and entered key) and a network outage shouldn't strip that choice. Smart Features is opt-in to begin with; the skip-on-picker affordance already covers "I can't or won't set this up right now." A separate configured_unverified state for Smart Features would add a fourth state to runtime gate logic without serving a customer flow that the picker-Skip doesn't already cover.

Quitting Pane mid-setup (after committing to a sub-flow but before submit) leaves `onboarding_completed_at` null. Next launch routes through §6.3A predicate (no longer needs a special Smart Features re-entry check — see `02-spec-onboarding-renumbers-and-touches.md` Edit 1) and the state machine resumes at §6.4.1 picker. The customer can re-try a sub-flow or Skip.

### 6.5 Completion + welcome message
> **Status:** NOT BUILT (sub-task #3 templated welcome; #2 cascade-UX unverified-state silent retry)

Onboarding completes when the customer reaches Home for the first time post-sign-in. There is no celebration screen; the transition is a direct drop into Home.

#### 6.5.1 Three valid completion states

| Primary `provider_state` | Triggered by | Banner | Welcome message |
|---|---|---|---|
| `configured_verified` | Primary provider validated successfully (§6.3.x or §6.6.x). Customer then either Submitted or Skipped at §6.4.1 picker. | None | Yes (per §6.5.3) |
| `configured_unverified` | Primary provider key saved but validation hit a network error and customer chose Skip on §6.3.x error path. §6.4 never reached. | Persistent banner: "Connect AI to enable chat → Settings" | None |
| `skipped` | Skip on the §6.2 provider picker. §6.3.x, §6.4, §6.6.x all never reached. | Persistent banner: same copy | Static setup nudge (per §6.5.3) |

**Internal-provider state is orthogonal.** `internal_provider_state` is set independently at the §6.4 finish line. Values:

- `configured_verified` — customer completed a §6.4 sub-flow and Submitted with at least Extraction picked.
- `skipped` — customer clicked Skip on §6.4.1 picker.
- null — §6.4 never reached (primary `configured_unverified` or `skipped`), or customer quit mid-§6.4 before Submit/Skip.

Internal-provider state does not influence welcome-message rendering or banner visibility in this table — those are primary-state-driven only. Internal-provider state controls runtime feature behavior per §6.4.6: `extraction_model` / `simple_tier_model` / `memory_search_llm_rerank_model` are null when `internal_provider_state ∈ {skipped, null}`, and each feature reads its own model setting; null means that feature is silently disabled at runtime.

A customer reaching Home with primary `configured_verified` and `internal_provider_state = skipped` is a valid completion. No banner, welcome message renders, three Smart Features-driven features silently disabled at runtime. Chat works via primary throughout.

Banner copy is unified across `skipped` and `configured_unverified` — internal state differs (skipped = empty key field; unverified = saved key + "Last validation failed" inline note in Settings) but the customer-facing message is the same. Distinguishing them in the banner would surface a state-machine concept the customer doesn't need.

#### 6.5.2 State writes at completion

`onboarding_completed_at` writes at the §6.4 finish line — submit on a §6.4 sub-flow or skip on the §6.4.1 picker. By the time the customer reaches §6.5 Welcome, the completion writes have already happened. §6.5 itself writes no completion state.

For the three primary-state cases (§6.5.1):

| Primary `provider_state` | Where `onboarding_completed_at` is written |
|---|---|
| `configured_verified` | §6.4.5 (sub-flow Submit) or §6.4.5 Path B (picker Skip). Customer always reaches §6.4 first. |
| `configured_unverified` | §6.3.x X6 case 2 Skip-to-unverified. Customer lands in Home with banner; the X6 case 2 path writes `onboarding_completed_at` directly per existing §6.3 spec. §6.4 not reached. |
| `skipped` | §6.2 picker Skip. Customer lands in Home with banner; the §6.2 Skip path writes `onboarding_completed_at` directly per existing §6.2 spec. §6.3, §6.4, §6.6 not reached. |

Quitting before any of these writes leaves `onboarding_completed_at` null. Next launch routes via the launch state machine and `onboarding_resume_state`.

> **Cross-reference note.** Existing §6.2 and §6.3 spec text may describe `onboarding_completed_at` as writing on Home arrival. Audit those references and update to read "writes on Skip" for the picker Skip path and X6 case 2 Skip path respectively, consistent with this table. If the existing spec already attributes the write correctly to those Skip paths (rather than to Home arrival), no further edit is needed there.

#### 6.5.3 Welcome message

> **Split-machine only (#2649).** OpenClaw is split-machine only; same-machine topology is not a supported configuration. The onboarding flow routes exclusively through manual gateway URL + pairing code entry; no topology choice screen, no same-machine detection, no local filesystem sync path.

> **Chat-in-Home gate chain (supersedes `provider_state` as the gate, #2564 B1 fix).** The actual mechanism that decides whether Home shows the personalized greeting or the static setup nudge is not the `provider_state` table above directly — it runs through a separate gate at chat-load time:
>
> 1. `onboarding_completed = 'true'` (`ai_settings`) triggers the greeting flow in `ChatSidebar.svelte`.
> 2. `aiLoadConfig()` → `ai_load_config` (`notes.rs`) → `derive_active_provider_label` (`runtime_resolution.rs`) — checks whether an active `runtime_type='openclaw'` agent exists, else falls back to the default agent's `runtime_type`, else reports `'none'`.
> 3. If the derived provider is empty/`'none'` → static "set up AI in Settings" welcome message.
> 4. If a provider is derived → personalized "Hey there, I'm {agentName}" greeting, via `resolveGreetingAgentName()`.
> 5. `greeting_sent = 'true'` prevents re-seeding on subsequent loads.
>
> This replaced the retired `ai_settings.provider` field as the source of truth (ADR-0063 D8) in the #2564 B1 fix; `provider_state` (as referenced through this section) governs the onboarding *completion* banner/state, not the Home chat-load gate itself.

§6.5.3 governs the Home first message across all provider states. In every state the first message is a **static seed, not an AI call** (§29).

**`provider_state = configured_verified` → fixed greeting.** Pane seeds the fixed greeting as the first message in the Home channel. Real chat message — persists, scroll-back-able, may trigger memory extraction on customer reply. Identical in mechanism across all provider paths. See “Post-onboarding greeting (#1223)” for the template text, variable source, and fallback.

- **All paths (OR, Anthropic, Ollama, OC — SplitMachine only):** the same fixed template — `Hey there, I’m [agent]. Very nice to meet you. What would you like to do first?` No path generates the greeting from a model. The prior per-path split (BYOK templated / OC agent-voiced from `SOUL.md` + `IDENTITY.md`) is removed per #1541: agent-voiced generation was the confabulation source, and identity/persona text must not be improvised on the first-impression surface. Persona files (`SOUL.md`, `IDENTITY.md`) remain correct and continue to drive all subsequent (real) messages — they are simply not used to produce the greeting.

**No provider / `provider_state = skipped` (provider null / ‘’ / ‘none’) → static setup nudge.** Pane seeds a fixed static setup message as the first message in the Home channel — **not** a bare empty state. It names the providers so a first-run customer knows how to enable chat:

> “Welcome to Pane! To start chatting, set up an AI provider in **Settings → AI**. You can use OpenRouter, Anthropic, or a local Ollama server.”

The “Connect AI to enable chat → Settings” banner remains visible until a provider is configured. This preserves the built #1227 behavior (the message §1227 previously carried); exact built copy to be confirmed at the code PR — GREP-VERIFY — and preserved, not replaced.

**`provider_state = configured_unverified` → banner + silent retry, no seed.** The customer has already saved a provider key that failed validation. The chat panel shows the empty state with the banner, and §6.5.4 silent retry runs. The setup nudge does **not** apply here — a “set up a provider” message would misfire for a customer who already has one — and the greeting does not seed until the provider verifies.

#### 6.5.4 Silent retry for unverified state

A customer in `configured_unverified` has a valid key but a failed validation. Per X6 retry rules, Pane silently re-runs the validation:

- On app open (Pane process launch after `configured_unverified`)
- On chat-screen open (customer navigates into the chat panel)
- **NOT on first message send.** This avoids mid-message validation surprises — the customer types a message, hits send, and would otherwise see a validation popup interrupt the send.

Banner clears silently on pass; stays visible with retry on fail. Cascade-UX (#873) handles this; sub-task #2 verifies the existing logic covers the unverified-provider state for v1.

#### 6.5.5 Quit-mid-onboarding

`onboarding_completed_at` stays null until the §6.4 finish line — Submit on a sub-flow (§6.4.2 / §6.4.3 / §6.4.4) or Skip at the §6.4.1 picker. Quitting before that point leaves `onboarding_completed_at` null; next launch routes via `onboarding_resume_state` and resumes from where the customer left off. State persists across app restart, OS restart, even crash. No data loss, no forced restart. See §6.5.2 for the full completion-state table.

#### 6.5.6 Seeding — behavioral contract (successor)
> **Status:** SPEC’D — #2030 / ADR-0060. Successor to the obsolete greeting/seeding section (retained as an anchor tombstone). Shared behavior is defined in agents.md §24.10.1a–e; this subsection states the onboarding-specific points.

**Two-document workshop.** The review workshop covers **Memory → Soul** only. Soul is the final step. The step indicator reads “1 of 2” / “2 of 2”. There is no Rules workshop screen and no “Moving to Rules Document” transition; RULES.md is not authored (ADR-0060 D3). The navigation-context prompt is otherwise unchanged — Soul was already the final step and Rules was only ever reached by the user clicking through, so removing it changes the step count and the transition marker, not the workshop’s generation prompt.

**No default project on any path.** Fresh install creates zero user projects (default-project.md, retired-creation). The upload and questions paths no longer create an invisible project; all onboarding paths converge on zero user projects. Workshop docs live in the sentinel scope (agents.md §24.10.1e), not a project.

**Uploads.** Raw uploaded documents land in the per-agent `.pane/system/{UUID}/uploads/` (agents.md §24.10.1b). The global `~/Pane/.pane/ai/uploads/` directory is retired entirely — no raw upload is written there. Document *contents* are synthesized into MEMORY.md and SOUL.md via the model (§24.10.1c); uploads are not listed by filename.

### 6.6 OpenClaw Integration
> **Status:** PARTIAL. Existing OC integration (sync architecture, gateway endpoints, fingerprint pinning) is BUILT through v0.65.5. Onboarding-specific flow (this section) is NOT BUILT — sub-tasks #879–#884.
> **Updated:** 2026-05-07 — full rewrite for redesign. Old "Enter gateway URL + auth token" narrative retired. Sync architecture (push/pull/initial) preserved; reference §22C for transport details.

Customers who choose OpenClaw at the picker pair with a gateway using manual URL + token entry. OpenClaw integration uses the SplitMachine flow exclusively — no topology choice screen.

> **Spec routing:** This section covers the onboarding screens and the high-level transport claims they make. The full transport details — gateway pairing protocol, auth/token model, TLS fingerprint pinning, sync endpoints — live in `docs/spec/sync.md` §22C. The new §22C.22 "Onboarding Integration" sub-section bridges the screens here to the gateway operations there.

OpenClaw onboarding is gated on §6.6.3 / #880 — provider card disabled until SplitMachine flow is built.

#### 6.6.3 SplitMachine flow
> **Status:** NOT BUILT (sub-task #880 — combined "Verify and connect" screen)

> Status (v0.65.25): §6.6.3 — BUILT

> Status: BUILT (v0.61.1, PR #1311) — TLS hostname extracted from gateway /v1/health during pairing; URL rewritten from IP to Tailscale hostname

> **Amended (2026-08-22, #2642/#2630 — architecture reversal).** The "Settings inline pairing flow" referenced throughout this section (§22C.11A.5) no longer runs inside `ProviderConnectionCard.svelte`. It now runs inside `NewProviderModal.svelte` (the "New Provider" modal), which owns the full OC ceremony end-to-end and can be entered either from the provider picker (fresh connection) or via a `startCeremony: 'pair' | 're-pair'` deep-link prop set when the user clicks "Pair gateway"/"Re-pair" on the (now manage-only) card. The shared `ocPairingCeremony.ts` module this note describes is unaffected — both onboarding and the modal still call the same shared ceremony functions; only the modal, not the card, is the Settings-side caller now. See `docs/spec/sync.md` §22C.11A.5's matching amendment.

> **Status (#2649, PR 3A):** the pairing-ceremony mechanics this section describes (steps 1-3: URL entry, verify-and-connect, initial sync) are now implemented via a single shared module, `src/lib/services/ocPairingCeremony.ts`, called by both this onboarding flow and the Settings inline pairing flow (§22C.11A.5 / `NewProviderModal.svelte`, formerly `ProviderConnectionCard.svelte` — see amendment above) — previously each screen reimplemented the ceremony independently and had drifted (Settings had first-pair/re-pair routing and wrote `gateway_paired_at`; onboarding had neither). Onboarding now also adopts:
> - **First-pair/re-pair routing** (`oc_agent_row_exists` — a DB row-existence check, not `info.connected`) before Step 3's initial sync: first pair still calls `sync_initial_import`; re-pair (re-onboarding onto an install that already has OC agent rows, e.g. after a prior disconnect) instead calls `check_oc_agent_collisions` then, gated on the result, `reimport_oc_agents` — exactly the routing Settings already had (see `docs/spec/sync.md` §22C.22.8 and the #2647/#2648 D18 fix this mirrors).
> - **`gateway_paired_at`** is now written on this path (previously only the Settings re-pair flow wrote it).
> - The redundant second `ai_configure_and_save` call and the redundant `fetchOcModels()` call that Step 3's old `runOcSync()` made after the initial sync (re-doing work the pairing ceremony already did in Step 2) are removed.
> - The dead `provider_state = configured_verified` write (§22C.22.5 documents this key; it has zero readers anywhere in the codebase) is removed from this path. See §22C.22.5's note for the corresponding sync.md status update.
>
> `ensureMainAgent` (post-ceremony agent-name-collection UX, screen `agent-names`) is unaffected — it runs after the shared ceremony completes, exactly as before; PR 3A's scope was the ceremony itself, not what onboarding does with its result.
>
> **Status (#2649, PR 3B).** The same-machine screens referenced above as "out of scope, tracked separately" (`openclaw-detect`, `oc-agent-discovery`, `AgentDiscovery.svelte`) are now deleted entirely. OpenClaw is split-machine only — same-machine was never an allowed topology and running OC on the same machine as Pane violates the isolation boundary that makes split-machine deployments secure. No live install carried same-machine state, so removal required no migration. See §6.5.3's split-machine-only note.

Prerequisite chain (per Q2): OC installed → Tailscale on both machines → Tailscale connected → Gateway DMG installed on OC machine → Pane DMG installed on user machine → onboard with Gateway URL. Customer is expected to have these in place before reaching this flow. Setup help (sub-task #932) covers the prerequisite documentation including ssh-via-Tailscale for headless OC machines (sub-task #23).

**Step 1 — Gateway URL entry.**

```
Connect to your OpenClaw machine

URL of your OpenClaw machine
[ http://100.64.0.1:18789                                       ]

Make sure OpenClaw + gateway + Tailscale are running on both
machines. [Setup help →]

[Continue]                                                  [Back]
```

URL field auto-prepends `https://` and defaults `:18789` if missing (sub-task #18). On Continue, Pane probes the URL — must distinguish "unreachable" from "reachable but not a gateway" (sub-task #17, protocol check after TCP connect succeeds). Errors per the error-table.

**Step 2 — Verify and connect (combined screen, per design brief screen F).** Single stacked-section screen combining fingerprint verification and pairing-code exchange:

```
Verify and connect

We're connecting to OpenClaw at [URL].

TLS fingerprint
SHA256: AB:CD:EF:01:23:45:...

Compare the fingerprint shown above against the one Pane Gateway
emitted to its terminal on startup (on the OC machine). If they
match, accept. If not, click "Fingerprint doesn't match".

Pairing code
[ 8-digit field ]   (static copy: "codes expire after 5 minutes")

Where to find these
- TLS fingerprint and pairing code are emitted by Pane Gateway to
  its terminal on the OC machine when the gateway starts up.
- If the pairing code expired (5-minute window from gateway
  startup), run on the OC machine: `pane-gateway pairing-code`.
  The new code has its own 5-minute expiry.
- The TLS fingerprint does not expire and does not need to be
  re-fetched — the cert persists at `~/.pane-gateway/gateway.crt`
  (default; configurable via `GATEWAY_DATA_DIR` env var).
- Setup help, including ssh-via-Tailscale for headless OC machines:
  [Setup help →]

[Connect]   [Fingerprint doesn't match]                     [Back]
```

**Obtaining the pairing code and TLS fingerprint.** Pane fetches the TLS fingerprint over the wire from the gateway's `/v1/health` endpoint at URL-probe time and displays it on screen. To verify, the user reads the fingerprint that Pane Gateway emitted to its stderr on startup (visible in the terminal where `pane-gateway` is running on the OC machine) and visually compares it against what Pane is displaying. If they match, accept; if not, hard-block.

The pairing code is different: it's not fetched over the wire. Pane Gateway emits the 8-digit pairing code (5-minute expiry) to stderr alongside the fingerprint on startup. The user reads the code from the gateway's terminal and types it into Pane's pairing-code input field.

**If the code expires before the user finishes entering it** (5-minute window starts at gateway startup), the user runs `pane-gateway pairing-code` on the OC machine to generate a fresh code. The new code has its own 5-minute expiry. The TLS fingerprint does not expire and does not need to be re-fetched — it's stable across the gateway's lifetime because the cert persists at `~/.pane-gateway/gateway.crt` (default; configurable via `GATEWAY_DATA_DIR` env var).

The `openclaw` CLI is not involved in pairing. Pairing is between Pane and Pane Gateway, both of which are Pane-owned components; OC is the downstream service the gateway proxies for. Earlier drafts of this section referenced `openclaw gateway pairing-code` and `openclaw gateway fingerprint` — those commands do not exist and were a spec error.

Two operations in one screen; the customer accepts the fingerprint and submits the pairing code in a single Connect action. On Connect:

1. Pane verifies the fingerprint matches what the gateway returned at probe time. Mismatch → §22C.4.3 hard-block (no Continue path, customer must abort and re-pair).
2. Pane calls `POST /pair` with the entered code (capture: external-contract-captures §1).
3. On success, gateway returns a 90-day bearer token + `expires_at` + `tenant_id`. Pane stores `gateway_token` (encrypted), `tls_cert_pin`, `gateway_url`, `paired_at`, sets `provider_type = openclaw`, `topology = splitmachine`.

**Pairing-code semantics (capture §3 informs UX):**
- Code is 8-digit numeric, generated cryptographically (`OsRng` per #722; transitional `thread_rng` pre-#722).
- Code TTL is 5 minutes from generation.
- Code is one-time-use; on success it's invalidated.
- Wrong code does *not* invalidate the active code — customer can retry on typos within the 5-minute window.
- Generating a new code on the OC side overwrites any previous active code (single-slot).

Error mappings on `POST /pair` (capture §2 — only `Unauthorized(_)` reachable from this handler; messages distinguish sub-cases):

| Wire `message` | Customer copy |
|---|---|
| `"Pairing code expired"` | "Code expired. Generate a new code on your OC machine." |
| `"Invalid pairing code"` | "Code didn't match. Try again or generate a new one." |
| `"No active pairing code"` | Same as expired — guidance is to generate a new one. |
| Network error (any layer below `/pair`) | Actual error string from gateway (e.g. `"Pairing failed: error sending request..."`). Fallback if empty: `"Pairing failed — check that the gateway is reachable and the pairing code is correct."` [Retry] |

**Step 3 — Initial sync.** After successful pairing, `GET /v1/sync/initial` pulls every agent's whitelisted identity files. Per Q-decision, v1 ships retry-all-or-fail-pairing on any sync failure (sub-task #21); partial-success state is not maintained. Sub-task #25 (graceful partial-sync handling) is filed POST-LAUNCH.

The sync screen presents a progress indicator. On failure: "Sync failed. [Retry] [← Back]" — Retry re-attempts the import; Back navigates to the pairing code entry screen (`oc-verify-connect`) and clears `ocSyncError` + `ocSyncing` (but preserves `ocFingerprint`, `ocCertPem`, `ocToken` from successful pairing). No skip path in v1.

**Step 4 — Your Team display.** §6.6.5.

**Step 5 — Drop into Home with agent-voiced welcome.** No "point OC to Pane" screen for SplitMachine — gateway sync handles bidirectional file movement, OC needs no workspace reconfig in SplitMachine (decision OC-C, source-read confirmed via Brit sub-task #22).

> **Note on retired SameMachine "Point OpenClaw" screen.** The pre-redesign flow had a "Point OpenClaw to Pane" screen at the end of OC onboarding for the SameMachine case. With the redesign, SameMachine relies on symlink + agent-discovery at startup (§24.0); SplitMachine relies on gateway sync. Neither needs the customer to run an `openclaw config set` command on the OC side at onboard time — that step retires.

#### 6.6.4 Identity File Whitelist
> **Status:** BUILT. PRESERVED VERBATIM from prior §6.5.4 — verdict SURVIVES per Reed's audit 2026-05-07.
> **Annotated 2026-07-28 (#2030 / ADR-0060):** Whitelist membership governs *sync*, not local generation. Locally-created (non-OC) agents generate only IDENTITY.md, SOUL.md, MEMORY.md (ADR-0060 D1); the remaining whitelisted files are OC-authored and arrive via sync. RULES.md in particular is no longer generated by onboarding (ADR-0060 D3) but its sync entry is retained — OC may still sync one over.

Only these files are synced between Pane and OpenClaw:

| File | Purpose |
|------|---------|
| SOUL.md | Agent personality and behavior |
| MEMORY.md | Long-term memory index |
| IDENTITY.md | Agent identity metadata |
| USER.md | Information about the human user |
| TOOLS.md | Tool configuration and notes |
| AGENTS.md | Agent operating rules |
| HEARTBEAT.md | Periodic check instructions |
| RULES.md | Additional constraints |
| INVARIANTS.md | Architectural hard constraints |

Work artifacts (scripts, builds, drafts, SQL dumps) are never synced. They belong in task workspaces (§17.6), not in `.pane/system/`.

#### 6.6.5 Your Team display
> **Status:** NOT BUILT (sub-tasks #883 read-only display, #275 hide picker for OC users)

After pairing + initial sync, Pane shows the discovered agents:

```
Your Team

Main: Brit Freed
Other agents: designer, Researcher, Stia (work behind the scenes)

To change main agent, edit OpenClaw config.

[Continue]                                                  [Back]
```

Display-only. No agent picker, no radio buttons — the main agent is determined by OpenClaw's config (`openclaw.json`), not by user selection during onboarding. The "main agent picker" UI from the pre-redesign flow retires (sub-task #883). Sub-task #882 ensures the model picker is similarly read-only for OC users (model is OC's choice, not Pane's).

The help-text link routes to documentation for editing OC config; it does not open an in-app editor.

To change which agent is your main agent, edit your OpenClaw config — Pane reads the main-agent assignment from OC's `agents.json` and does not maintain its own. [How to change main agent →](https://paneapp.ai/help/openclaw#main-agent)

#### 6.6.6 OC-mode model resolution — agent registry as source of truth
> **Status:** BUILT (v0.65.74, PR #1322)
> **Status:** BUILT (v0.65.77, PR #1353) — hotfix: oc_agents_url helper strips trailing /v1 before appending /v1/agents; ai_configure_and_save now passes raw endpoint to fetch_oc_agent_model. (#1345)
> **Status:** BUILT (v0.67.9, PR #1486) — bugfix: `ai_configure_and_save` now restores previous `state.provider` on error via `ProviderRestoreGuard` drop-guard instead of leaving `AiProvider::None` permanently (which blocked chat via #1460 guard). Frontend `submitPairingCode` resets `pairingStep` on all failure paths; step 3 template now shows errors + cancel. (#1485)

In OC mode (`provider: "openclaw"`), Pane resolves the AI model by calling `GET /v1/agents` on the configured gateway and reading the `model` field of the agent whose `oc_agent_id == "main"`.  **Zero hardcoded fallback strings are permitted in this path.**

- `ai_configure_and_save` (in `src-tauri/src/ai/connections.rs`) calls `fetch_oc_agent_model` when the incoming `model` argument is absent or equals the routing sentinel `"openclaw"`.  If the gateway is unreachable, returns 4xx/5xx, or returns no `model` field, the call returns a clear user-facing error and the configuration is NOT saved.  On error, the prior `state.provider` is restored (not left as `AiProvider::None`) via a `ProviderRestoreGuard` drop-guard to prevent the #1460 async-race guard from permanently blocking chat.
- The model stored in the DB (and sent in every subsequent chat request) is the real value from the gateway — e.g. `"anthropic/claude-opus-4-7"` — never the sentinel `"openclaw"` or any baked-in literal.
- G3 (`forbid-hardcoded-constants.yml`, mode: `fail`) is the regression guard.  Any future `"anthropic/claude-*"` literal introduced in production Rust outside `context_budget.rs` will fail CI.  See `docs/dev-hygiene-playbook.md` Appendix E §G3.

In BYOK mode (OpenRouter / Anthropic / Ollama), the model is supplied by the user picker.  This section does not apply to BYOK paths.

### 6.7 Re-onboarding cases (X8 reference)
> **Status:** Cross-cutting; specific sub-task assignments below.

Three cases where a customer re-enters some part of onboarding:

**Case 1 — Customer wipes Pane data.** Full re-onboarding. Treat as first-run. `install_uuid` is regenerated (per ADR-0026); backup metadata written from then on carries the new UUID. Nothing is reported anywhere.

**Case 2 — Customer migrates to new machine.** Backup-restore offered at §6.1.4. If accepted, picker is skipped; customer lands in Home with restored data.

**Case 3 — Customer adds an OC machine for SplitMachine after BYOK onboarding.** Run the OC-side onboarding from the OC machine (covered in OC's documentation, not Pane's). On the Pane side, the customer changes provider in Settings via cascade-UX (§22C.11A.3); inline pairing flow runs in Settings (§22C.11A.5), not via re-entering this onboarding wizard. No new branch in the onboarding flow.

Multi-machine sync is not a shipping feature. The account/sync engine that would carry it is present but behind `account_enabled` (§6.0, ADR-0070). On OC paths, gateway-side single-Pane pairing rejection (sub-task #7) still applies. The former single-active-machine enforcement (§6.1.3) is retired; no onboarding/help copy mentions machines.

---

## End of §6 replacement.

---

## Post-onboarding greeting (#1223, v0.65.54)

> **Amended #1541 (fixed template):** the greeting is a fixed string, not a model-generated introduction. It is **not an AI call** — it does not call `ai_chat_stream` for generation and does not dispatch through either provider path (§29). No smart-routing decision applies (smart-routing.md §27.6.5 is retired). See the ADR-0039 supplement (2026-07-15).

After onboarding completes, `ChatSidebar.svelte` seeds a fixed greeting as the first message in the Home chat:

1. **Fixed template, not generated.** The greeting text is the fixed string:
 > Hey there, I'm [agent]. Very nice to meet you. What would you like to do first?

 `[agent]` is the main agent's display name, read from `ai_agents.name WHERE is_main = 1`. If the name is empty or unavailable, the fallback is `Pane` (unchanged from ADR-0039). The agent does not improvise its own introduction; the template contains no persona claims, so the greeting cannot confabulate identity or personality text — this is the #1541 fix. IDENTITY.md still carries the name into the system prompt for all subsequent (real) messages.

2. **Persists as the first assistant message.** The greeting is saved and displayed as a `role: 'assistant'` message and remains in the thread (scroll-back-able) — it is **not** removed after render. The prior behavior (generate, then hide from history) is removed.

---

## Project greeting (#1541)

> **Status:** SPEC'D — new for #1541. No project greeting existed previously; project chats showed only the static fresh-session hint (`FRESH_SESSION_HINT`) with no agent message.

When a project's chat thread is opened with **no prior messages** (empty thread), Pane seeds a fixed greeting as the first message in that thread. Like the Home greeting, it is a fixed template — **not a model call** — and does not dispatch through either provider path (§29).

**Text:**
> Hey there, I'm [agent]. I'm here to help with [project-name]. Where would you like to start?

**Variables:**
- `[agent]` — main agent display name, `ai_agents.name WHERE is_main = 1`. Fallback `Pane` if empty/unavailable (per ADR-0039).
- `[project-name]` — the project's name, `projects.name` (via `context.projectName`).

**Fire trigger (behavioral rule):** Evaluated **when a project's chat is opened** — i.e. the user navigates into that project's chat and its thread loads. This is the project analogue of the Home greeting's `pane-onboarding-complete` trigger. At that moment Pane checks the empty-thread condition: if the thread for that `project_id` has zero user/assistant messages, seed the greeting as the first message; otherwise do nothing. Seeding makes the thread non-empty, so on any later open the condition no longer holds and the greeting does not re-fire. The trigger is the empty-thread state at open time, not a persisted per-project "first-ever" flag (self-healing, no new schema). If a user later clears a project thread back to empty, the next open re-seeds the greeting — accepted: a fresh project thread gets a fresh greeting.

**Render:** Saved and displayed as a `role: 'assistant'` message, the first message in the project thread. Renders as an agent bubble with the agent-name header — same mechanism as the Home greeting. No render changes.

**Relationship to the fresh-session hint:** The project greeting *supplements* `FRESH_SESSION_HINT`, it does not remove it. Once the greeting seeds the first message, the thread is no longer empty, so the hint (shown on empty/fresh threads) does not display for that thread. The hint's behavior is unchanged wherever no greeting seeds.

**Fallback (cannot render broken):** If `[project-name]` does not resolve (missing/empty project context), Pane does **not** render `"...help with ."` — it skips the greeting and falls through to the existing `FRESH_SESSION_HINT`. If `[agent]` does not resolve, the `Pane` fallback name is used and the greeting still renders. The greeting seeds only when a valid `[project-name]` is present.

---

## Workshop screen — note persistence and user editing (#1225, v0.65.55)

Before entering the workshop screen, the component calls `ensureNote()` for both documents (Memory, Soul). This function:
1. Searches existing notes in the **workshop sentinel scope** (agents.md §24.10.1e — a reserved `project_id` not backed by a `projects` row; no default project exists) for a note with the matching title.
2. If found, updates its content and returns the ID.
3. If not found, creates a new note and returns the ID.

The returned IDs are stored in `memoryNoteId` and `soulNoteId`. The AI receives these IDs in its navigation context and uses `edit_note` to apply changes — this requires the IDs to be populated before the screen renders. (Rules is no longer authored — ADR-0060 D3.)

**User editing:** The left panel renders a `<textarea>` (not `{@html marked(...)}`). The textarea binds one-way via `value` attribute and updates the correct content variable on `oninput`. A 500ms debounce (`debouncedSave()`) writes the updated content to the DB via `update_note`. During AI animation (`animatingDoc && displayContent`), the read-only markdown view is shown instead so the streaming diff animation works correctly.

Both user edits and AI edits are supported simultaneously — AI changes update the content variables via note polling, and user changes write through the debounce path.

**Workshop note cleanup (v0.68.54, #2213; RULES removed, sentinel scope #2030):** On completion (`saveAndComplete`), after system files are written to `.pane/system/{UUID}/` via `ensureAgentFile()`, both workshop notes are purged via `purge_note()`. The notes exist only for the duration of the workshop in the sentinel scope (§24.10.1e) — write-through scaffolding so the AI can use `edit_note`, not permanent user notes. Purge-at-completion is required so nothing user-visible is left behind (#2216 discipline).

**Transcript persistence param fix (#2308, STATUS: FIXED):** `saveAndComplete`'s transcript-save block previously called `invoke('save_chat_message'/'load_chat_messages')` directly with `{ projectId, role, content }` / `{ projectId, limit }` — omitting the required `threadId` (save) and `sessionId` (load) keys the Rust commands (`src-tauri/src/ai/chat.rs`) actually expect, so every save/verify call failed silently at the Tauri invoke layer. Now routes through the shared `api.ts` wrappers (`getActiveSession`, `sendChatMessage`, `saveAssistantMessage`, `getChatMessages`) used by every other chat-persistence call site, which resolve a real thread id and pass the correct keys.

**Tool call result parsing (v0.68.23, #2049):** When the AI stream completes with tool results, the done handler must extract `note_id` from tool call arguments using the canonical backend format `{ id, name, params }`. All three argument key paths must be checked: `tc.function?.arguments` (legacy/provider format), `tc.input` (alternate provider format), and `tc.params` (canonical format). If `note_id` is successfully extracted and matches one of the workshop note IDs, the handler fetches the updated note content from the backend and refreshes the corresponding Svelte state variable (`memoryContent`, `soulContent`, or `rulesContent`). This refresh is critical because `saveAndComplete` reads these state variables when writing the final system files — stale state means the AI's edits are silently discarded.

**Workshop chat session (#2618, STATUS: FIXED):** The workshop's chat rail calls `ai_chat_stream` to let the AI refine the Memory/Soul documents. Prior to this fix, that call passed `context.projectId = WORKSHOP_SCOPE_ID` (the DB-only-notes sentinel described above) but no `sessionId`. Post-ADR-0062, `ai_chat_stream`'s dispatch is session-first: with no explicit `sessionId`, the backend's fallback path (`ai::chat`) resolved (and then treated as a literal session id) the sentinel string itself, which never existed as a `chat_sessions` row — `agents.rs::resolve_and_selfheal_session_agent` threw `"Session __workshop__ does not exist"` before the AI could respond, for every brand-new agent in both onboarding and Settings → Create Agent.

`WorkshopReview.svelte` now takes a required `sessionId` prop, threaded directly into `ai_chat_stream`'s top-level `sessionId` argument (the same pattern `ChatSidebar.svelte` uses for its own session-bound chat), which bypasses the buggy fallback path entirely. Both callers (`Onboarding.svelte`'s `processUpload`/`processQuestions`, and `AgentCreateOverlay.svelte`'s `handleUploadConfirm`/`handleInterviewFinish`) resolve this session **before** entering the workshop screen, via a new shared helper: `createWorkshopSession(agentId)` (`src/lib/onboarding/workshopSession.ts`), which calls `api.createSession(WORKSHOP_SESSION_SCOPE_ID, agentId)` to create a real, disposable `chat_sessions` row explicitly bound to the just-created/just-resolved agent.

`WORKSHOP_SESSION_SCOPE_ID` (`constants.ts`) is a distinct sentinel from `WORKSHOP_SCOPE_ID` above — the latter remains notes-only (DB-only workshop notes, no backing `projects` row); conflating the two would misuse a notes-scoping concept as a session id again, the same category of bug this fix resolves. `api.createSession` (not `api.getActiveSession`) is used deliberately: `getActiveSession` would silently reuse a pre-existing session without rebinding its immutable `agent_id`, which in the create-agent-from-Settings flow (where a prior workshop session may already exist from an earlier agent) would refine the *wrong* agent's Memory/Soul. Session rows created under this scope are never explicitly cleaned up — this mirrors the app's existing "New Session" behavior elsewhere (inactive `chat_sessions` rows persist after a new session is created) and is harmless, since nothing lists or surfaces sessions scoped to `WORKSHOP_SESSION_SCOPE_ID` in the UI.

**Agent-before-synthesis ordering invariant (#2634, D10, STATUS: FIXED):** `ensureMainAgent()` (which calls `upsert_agent`, creating the first `ai_agents` row) **MUST** be called and awaited **before** any `streamAndCollect()` / `ai_chat_stream` invocation in every onboarding path — `processUpload` and `processQuestions` both build an extraction prompt and then need a real agent to dispatch against. Agent creation is a *precondition* for synthesis, not a post-step: `ai_chat_stream`'s dispatch resolves session→agent via `resolve_and_selfheal_session_agent` (`agents.rs`), which returns a hard `Err("No agent is set up yet...")` when zero `ai_agents` rows exist — the exact state before the first agent is created on a fresh install. Both functions previously called `streamAndCollect(extractionPrompt)` before `ensureMainAgent()`, so every fresh-install upload/questions run hit this `Err` immediately; `processUpload`'s catch block then mislabeled it as a generic "Processing took too long" (logged correctly to console but shown to the user as a fabricated timeout message), while `processQuestions`'s catch block didn't even log it. Fixed by moving `ensureMainAgent()` (and the `ocMainAgentName`/`ocMainAgentId` assignment) above the `streamAndCollect()` call in both functions — the same ordering `AgentCreateOverlay.svelte`'s `handleCreate()`→`handleUploadConfirm()` sequence already used correctly (agent created in Step 1, synthesis only runs in Step 2 once `createdAgent` exists). `processUpload`'s fallback message now surfaces the real error (`Synthesis failed: <message>`) instead of the generic timeout text; `processQuestions`'s catch block now logs to console, matching `processUpload`. Verified no duplicate-agent risk from the reorder: the later agent-rename step (`proceedFromAgentNames`, `agent-names` screen) calls `update_agent` on the existing row, never a second `upsert_agent`/`create_local_agent`, so naming after creation cannot mint a duplicate; and `createWorkshopSession(agent.id)` (called after the reorder, same as before) still binds to the exact id `ensureMainAgent()` returned in the same function call — no id drift, covered by `Onboarding.workshopSession.test.ts`. See `docs/audits/2026-08-21-d10-upload-synthesis-ordering.md` for the full root-cause trace.

**Amended 2026-08-23, D26/PR-B (#2673):** `createWorkshopSession` now runs **before** `streamAndCollect`, not merely before the later workshop-review chat step. Root cause: `streamAndCollect` previously took no session identity at all, so `ai_chat_stream`'s dispatch fell back to whichever session happened to be active in that project scope — confirmed via live logs to dispatch create-agent synthesis through the WRONG agent's runtime (e.g. an existing OpenClaw-runtime agent instead of the newly created Anthropic-runtime agent). `streamAndCollect` now takes a **required** `sessionId` argument (`src/lib/onboarding/seedGeneration.ts`), threaded into `ai_chat_stream` as a top-level `sessionId` field (same pattern `WorkshopReview.svelte` already used for the review-chat step, §2618 above). Both `processUpload`/`processQuestions` (this file) and `AgentCreateOverlay.svelte`'s `handleUploadConfirm`/`handleInterviewFinish` now call `createWorkshopSession(agent.id)` immediately after the agent exists and **before** calling `streamAndCollect(extractionPrompt, workshopSessionId)` — one session, created once, used for both synthesis and the subsequent review chat. See `docs/spec/sessions.md` §30.7.1a for the normative session-binding rule this fix implements.

---

## Greeting edge cases — no provider and stream failure (#1227, v0.65.57)

> **Status: OBSOLETE as of #1541.** Both cases this section described are gone under the fixed-template greeting, and no unique normative content remains — the seeding mechanics are covered by "Post-onboarding greeting (#1223)" and the no-provider / skipped / unverified state by §6.5.1/§6.5.3. Heading retained for anchor stability. **Seeding behavioral contract** has moved to §6.5.6 (successor subsection, #2030 / ADR-0060).
>
> - **Case 1 (no AI provider):** the “check provider, skip the stream” mechanism is moot — there is no stream to skip in any path; the greeting is always a static seed. But Case 1’s user-facing static setup message is **preserved, not lost** — it now lives in §6.5.3 as the `skipped` / no-provider home message (with the banner). The mechanism moved (no stream check); the message stayed.
> - **Case 2 (stream failure):** unreachable. The greeting is not a model call and never streams, so there is no `ai_chat_stream` throw and no catch to handle. Retired.

*Historical (pre-#1541):* `handleOnboardingComplete` called `ai_chat_stream` to generate the greeting and handled two edge cases — a no-provider skip that showed a static setup message, and a stream-failure catch that showed a graceful fallback. Retained here as history only.

---

## Ollama minimum model recommendation (#1240, v0.65.57)

Ollama models under 14B parameters are not reliably capable of memory extraction or message classification. Two UX warnings were added:

**Onboarding (`smart-features-ollama-setup` screen):** After the Rerank Model picker and before the Submit button, a yellow advisory box reads:

> ⚠️ Model Size Recommendation — Smart Features work best with models 14B parameters or larger. Smaller models may not reliably extract memories or classify messages. We recommend `qwen2.5:14b` or larger.

**Settings (Smart Features section):** When `internalProvider === 'ollama'`, the same advisory appears below the model pickers:

> ⚠️ Model Size Recommendation — Models under 14B parameters may produce unreliable results for extraction and classification. We recommend `qwen2.5:14b` or larger.

---

## "What should I upload?" help link (#2311)

The Upload Documents screen (`screen === 'upload'`) has a "What should I upload?" link above the drop zone, opening `https://paneapp.ai/system-files-creation` via `openExternalUrl` (Tauri `plugin-shell` `open()`, matches AuthScreen/ChatPanel external-link pattern) — same link text/treatment as the create-agent "Add context files" screen (agents.md §24.10.1).

---

## Provider card fixed height + Smart Features descriptors (#2792)

Amends §6.2 and §6.4.1 (both above): provider cards previously carried no explicit height, letting per-card content (name, plus an optional descriptor) determine card height directly — which meant cards on the same row could render at visibly different heights, and (via an unrelated stylesheet defect fixed in the same change) sometimes rendered far taller than intended. Fixed by this ruling:

- **`.provider-card` uses a single fixed `height`** (52px), shared by every provider-card instance in `Onboarding.svelte` — both §6.2 (Connect Your AI) and §6.4.1 (Connect Smart Features) render from the same CSS class, so they cannot drift apart in height.
- **Name and descriptor both truncate with an ellipsis** (`white-space: nowrap; overflow: hidden; text-overflow: ellipsis`) rather than wrapping onto a second line. This is the mechanism that makes the fixed height hold regardless of provider-name or descriptor length — a long descriptor is clipped with `…`, not wrapped into a taller box.
- **§6.4.1 (Connect Smart Features) cards now carry the same one-line descriptor text as §6.2 (Connect Your AI)** for the three providers currently offered there: OpenRouter — "Access multiple models"; Anthropic — "Claude direct"; Ollama — "Local models". Previously §6.4.1 cards rendered provider name only, with no `.provider-desc` element at all, which was the actual source of the visible height mismatch between the two pickers before the fixed-height rule above made card height stylesheet-driven rather than content-driven.
- Decision D5 (Venice has no privacy/uncensored descriptor copy) is unaffected — Venice's §6.2 descriptor stays "Your Venice API models", and Venice is not one of the three §6.4.1 Smart Features providers.

Root cause note (implementation detail, not a spec contract): the pre-fix oversized-card symptom traced to a duplicate, later-in-source-order `.provider-card` CSS rule block in `Onboarding.svelte`'s stylesheet (a leftover merge artifact) that silently won the cascade and forced a full-width, no-height-cap column layout on every card in the file. That duplicate block — along with its dead `.provider-card.selected` state selector, never applied by any markup in this file — was removed rather than merged, since the earlier block was already the one every card's markup was written against.
