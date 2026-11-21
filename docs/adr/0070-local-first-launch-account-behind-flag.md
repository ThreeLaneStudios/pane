# ADR-0070: Pane opens to Home with no sign-in; account and sync stay behind a default-off flag

**Status:** Proposed (Matt, 2026-11-09)
**Date:** 2026-11-09
**Author:** Matt Carano (decisions), Claude (spec session)
**Supersedes:** ADR-0025 (retired in full), ADR-0027 (retired in full), ADR-0036 (retired in full); the entitlement halves of ADR-0026, ADR-0029 and ADR-0035 — see "Supersession map"
**Related:** ADR-0024 (local-only decryption — stands, strengthened), ADR-0002 / ADR-0004 / ADR-0014 (sync and encryption — unchanged), ADR-0044 (keychain-only secrets), §6 (`docs/spec/onboarding.md`), #3195, #3106

## Context

Pane is going open source (#3106). The ecom backend (`api.paneapp.ai`, the thin wrapper over Supabase + Stripe from ADR-0028) is being shut down: the core app, signed build and auto-updates become free, and extensions are the only paid line, later, through Stripe on paneapp.ai.

Today the app will not open until the user signs in. `src/routes/+page.svelte` renders `<AuthScreen />` whenever `!$isAuthenticated`; `initAuth` (`src/lib/stores/authStore.ts`) races `initAuthInternal` against a 5 s timeout and, on timeout, sets the session to `null` — which lands on the same gate, so the "local-only fallback" is not a mode, it is the sign-in screen. Once in, `checkEntitlementAuthenticated()` calls `auth_check_entitlement` at launch and on every `visibilitychange` / `tauri://focus` event (ADR-0025, 30 s debounce), routes non-`active` `subscription_status` to S7 and `active_machine_id` mismatches to S8 or silent sign-out (ADR-0029, ADR-0035, ADR-0036), and shows a grace-period toast. A public local-first repo that phones a billing backend before it opens is the first thing a reader would flag — and after shutdown it would not open at all.

The account is also the identity for encrypted CRDT sync through Supabase (ADR-0002, ADR-0004, ADR-0014). No multi-machine product ships today, but the engine is built and is the planned identity layer for team sharing and the phone app. Local data does not depend on it: the notes DB is plain SQLite, and the keychain master key (`sync_master_key`, ADR-0044 Addendum A) encrypts sync changesets only.

Two facts from the source-read shape the decision:

- **Sign-in is wrapper-bound.** `auth::sign_in` and `auth::email_check` POST to `{PANE_ECOM_URL | https://api.paneapp.ai}/v1/auth/*`. Only `refresh_session`, `sign_out`, `request_password_reset` and `change_password` talk to Supabase GoTrue directly. Keeping sign-in "as is" would keep dead code pointing at a dead host.
- **`signOut()` is destructive.** It calls `clear_encryption` (deletes the master key) and `clear_auth_session`, and on partial failure writes `cleanup_pending.json`, which `initAuthInternal` replays on the next launch. `initAuthInternal` also calls `clear_auth_session` when a stored token is expired and refresh fails. Any migration that runs that code path can delete the master key.

## Decision

1. **Pane opens to Home — Home chat plus the Jobs panel — on every launch, with no sign-in and no entitlement check.** The `AuthScreen` startup gate, `checkEntitlementAuthenticated()`, the launch + on-focus cadence, the grace-period toast, S7, S8, `auth_check_entitlement`, `auth_take_over_machine`, `auth_email_check`, and all consumption of `subscription_status` / `active_machine_id` / `subscription_action_url` are removed. First launch still shows the provider picker (§6.2) over Home when `onboarding_completed` is unset; that is the only screen between launch and chat.

2. **Account sign-in, session storage (§6.1.5), Supabase CRDT sync and Settings → Account stay in the codebase behind one flag, default off, compiled and tested in CI.** No shipped UI surface reaches them while the flag is off. They are present for planned team sharing and mobile; they are not a shipping feature, and the README and this ADR say so plainly.

3. **The flag is a runtime `ai_settings` row, `account_enabled`, with no UI.** `'true'` is on; absent, anything else, or a failed read is off. One reader per side:
   - Frontend: `initAuth` reads it first (via `get_ai_setting`). Off → `authSession` is set to `null` and `initAuth` returns without calling `initAuthInternal`; no 5 s race, no `getAuthSession`, no cleanup-flag read. Settings renders its Account block only when on. `startAutoSync` is only ever reached from `initAuthInternal` / `signIn`, so it is never called when off.
   - Backend: a single `require_account_enabled()` guard at the top of every `sync/commands.rs` command that touches session storage, the master key, or Supabase (`auth_sign_in`, `auth_sign_out`, `auth_refresh`, `auth_request_password_reset`, `auth_change_password`, `register_device_cmd`, `save_auth_session`, `clear_auth_session`, `init_encryption`, `clear_encryption`, `sync_configure`, `sync_push`, `sync_pull`, `sync_files_push`, `sync_files_pull`, `start_auto_sync`, `check_cleanup_pending`, `write_cleanup_flag`, `clear_cleanup_flag`). Off → `Err("account disabled")`. `get_auth_session` and `is_encryption_ready` are read-only and stay callable so tests can assert state.
   - The `import.meta.env.DEV && __PANE_TEST_MODE__` bypass in `initAuth` is removed: it existed to satisfy the gate, and the gate is gone. The e2e fixtures stop setting `__PANE_TEST_MODE__`.
   - Why a runtime row and not a compile-time `cfg` / Vite env: "compiled and tested in CI" means one binary, not two; a DB row is flipped by the existing `set_ai_setting` in tests and by Matt on his Mac to live-test the on-path without a special build; and it follows the existing `*_enabled` convention (`embeddings_enabled`, `link_previews_enabled`, `venice_system_prompt_enabled`). Nothing existing covers the concept — `tier3_topology` and `bridge_*` are OpenClaw gateway state, `sync_state` is an engine string, not a toggle.

4. **Flag-on sign-in targets Supabase GoTrue directly.** `auth::sign_in` moves from the wrapper's `/v1/auth/signin` to `{SUPABASE_URL}/auth/v1/token?grant_type=password` with the `apikey` header, body `{email, password}`, and parses the same token response `refresh_session` already parses (`access_token`, `refresh_token`, `expires_at`, `user.id`, `user.email`). `install_uuid` and `machine_name` are no longer sent on sign-in; `auth_email_check` and the S1a/S1b/S1c split go with the wrapper. The wrapper hop existed only to attach the entitlement snapshot and the machine claim, both of which this ADR removes. `PANE_ECOM_URL` and `get_wrapper_url()` are deleted.

5. **The migration is not a sign-out — master-key safety rule.** The flag-off build never calls `clear_encryption` or `clear_auth_session`, never reads or replays `cleanup_pending.json`, never runs `initAuthInternal`'s expiry/refresh branch, and — enforced by the same `require_account_enabled()` guard on `start_supabase_notification_poller`'s launch and on `push_notification` (Addendum B) — never reaches either command's `save_auth_session` write. After upgrade on an existing install: keychain `sync_master_key` present, `supabase_access_token` / `supabase_refresh_token` present, `~/Pane/.pane/auth_session.json` untouched, `cleanup_pending.json` (if any) left in place, `install-uuid.json` untouched, local DB untouched. If the flag is later turned on, `initAuthInternal` runs exactly as today, including cleanup replay and expiry/refresh — at that point the user has asked for the account path and its existing semantics apply.

6. **Nothing is deleted from the sync stack or the Supabase schema.** `sync/`, `keystore.rs`, `crypto.rs`, the `devices` / `changeset_queue` tables and their RLS policies stay. ADR-0002 / 0004 / 0014 are unchanged.

7. **Order of operations.** The flag-off build is installed and live-tested on Matt's Mac — app opens to Home, master key and session still present — before the ecom backend is switched off. `~/Pane` and the DB are backed up before the first install.

8. **Out of this ADR:** extension entitlement, pricing, Stripe, `docs/spec/paneapp-ai.md` and the website / ecom repos, #3158.

### Supersession map

| ADR | Disposition | What stands |
|---|---|---|
| ADR-0025 Ecom check cadence | **Retired.** | Nothing — there is no entitlement check to schedule. |
| ADR-0027 Pane ↔ Ecom API contract | **Retired.** | Nothing — the wrapper has no Pane-side consumer once sign-in targets GoTrue. |
| ADR-0036 Subscription-status routing matrix | **Retired.** | Nothing — no `subscription_status` is consumed. |
| ADR-0026 Install UUID | **Entitlement half superseded.** `active_machine_id` matching, send-at-sign-in, take-over confirmation, and the restore-must-preserve-UUID-for-take-over rationale go. | `install_uuid` generation, lifetime and reset rules stand: `backup.rs` writes it into backup metadata. |
| ADR-0029 Entitlement response shape + email-check privacy | **Entitlement half superseded.** The 5-value enum, `subscription_action_url`, retry-info fields, S7 CTA mapping, and the email-check response shape go with the wrapper. | The privacy posture (no account-derived metadata on an anonymous endpoint) stands as a principle for any future account surface. |
| ADR-0035 Active-machine mismatch call-site rule | **Entitlement half superseded.** Both surfaces and the discriminator go. | The rule that `clear_auth_session` is the only way to a clean no-session state (§6.1.5) stands. |
| ADR-0024 Local-only decryption | **Stands, strengthened.** | Decryption uses keychain credentials only; no ecom token is ever required. Points 2 and 4 (first-sign-in hard block, S7 as overlay) are moot — noted inline, not amended. |

## Consequences

### Positive
- The public repo opens to Home with no network call. The first thing a reader sees is a local-first app.
- One flag, one row, one reader per side. No divergent binaries; CI tests both states by flipping a row.
- Flag-on sign-in works against infrastructure that still exists (GoTrue), so the kept code is live code, not a fossil.
- No account data is destroyed on upgrade. Turning the flag on later finds the session and master key where they were left.

### Negative
- The flag is a DB row, so a determined user can flip it and reach an unfinished account UI. Acceptable: it is not a shipped surface, Settings says so, and the worst case is a Supabase sign-in that syncs nothing anyone else can see.
- 326 lines of §6.1 become history. 6.1.2 and 6.1.3 are reduced to retired stubs pointing here; git keeps the bodies.
- Existing `authStore.test.ts` cases for cleanup replay assume the on-path and must seed `account_enabled = 'true'`.

### Risks
- A consumer of `userEmail` / `isAuthenticated` / `authSession` that assumes a session exists throws with the flag off. Known consumers: `Settings.svelte` (inline Account UI ~L4439, `loadSyncInfo` gate ~L651), `syncStore.ts::schedulePull`, `_window-event-shim.ts`. `AccountSection.svelte` is imported nowhere and `api.authSignUp` invokes an unregistered `auth_sign_up` — both already dead; Reed's scope decides whether they go now or later. Red-first test 3 below covers the live consumers.
- Matt's stored session will likely be expired by the time the on-path is live-tested; `initAuthInternal` will try `auth_refresh` against GoTrue and, on failure, clear the session and show the Account block signed out. That is the on-path's normal behaviour, not a migration defect.
- The Supabase project URL and anon key are hardcoded in `auth.rs`. The anon key is designed to be public; whether it belongs in the public repo is for the #3106 security review, not this ADR.

## Alternatives Considered

- **Delete the account and sync stack outright.** Cleanest public repo. Rejected: the engine is the planned identity layer for team sharing and mobile, is built and tested, and nothing about it phones a billing backend. Deleting it to re-add it later is churn.
- **Compile-time `cfg(feature = "account")` + `VITE_PANE_ACCOUNT`.** Guarantees the shipped binary cannot reach the code. Rejected: two flags to keep in lockstep across Rust and Svelte, a second CI build to keep the code compiling, and the registered-commands list in `lib.rs` becomes conditional. The DB row gives the same default-off with one binary.
- **Keep sign-in wrapper-bound and let flag-on be mock-only.** Rejected: "kept" code with no server is dead code, and red-first test 4 ("flag on restores sign-in") could never pass against anything real.
- **Keep the 5 s local-only fallback as the default path.** Rejected: the fallback lands on the gate, and with the gate gone the race has nothing to race against.
- **Replay `cleanup_pending.json` on the first flag-off launch.** Rejected: a pending `cleanup_encryption: true` flag means a prior sign-out failed to delete the master key; replaying it deletes the key the migration promises to keep.

## References

- Related issues: #3195 (this change), #3106 (OSS prep)
- Spec section: `docs/spec/onboarding.md` §6 — new §6.0 Launch; §6.1 retitled and flag-gated; 6.1.2 / 6.1.3 retired; §6.7 multi-machine line amended
- Related ADRs: ADR-0024, ADR-0002, ADR-0004, ADR-0014, ADR-0044
- Supersedes: ADR-0025, ADR-0027, ADR-0036 (retired); ADR-0026, ADR-0029, ADR-0035 (entitlement halves) — status lines appended to each in the same PR
- Code touched by the build scope (not this PR): `src/routes/+page.svelte`, `src/lib/stores/authStore.ts`, `src/lib/stores/syncStore.ts`, `src/lib/components/AuthScreen.svelte`, `S8TakeOver.svelte`, `Settings.svelte`, `src-tauri/src/sync/commands.rs`, `src-tauri/src/sync/auth.rs`, `src-tauri/src/notifications.rs` (see Addendum A — its own private `auth_session.json` read/write pair is noted, not fixed, here), `src-tauri/src/lib.rs`, `e2e/fixtures.ts`, `tests/e2e/home-fixtures.ts`

## Addendum A — Build-time factual corrections (#3195 PR A)

**Date:** 2026-11-10
**Status:** Factual correction only — nothing else in this ADR is reworded.

Four corrections found while building PR A (the `account_enabled` guard, first of the three-way split of this ADR's implementation):

1. **Readers list gains two entries Decision 3 didn't name.** Decision 3's "one reader per side" undersold itself: `start_supabase_notification_poller` (`src-tauri/src/notifications.rs`) is a THIRD reader, on the backend side — it starts unconditionally at every launch today, gated only on a stored session existing, and its own refresh path makes a live Supabase call independent of the command-surface guard. PR A gates its start on the same shared `require_account_enabled()` the command guard uses. On the frontend side, Decision 3's claim that "`startAutoSync` is only ever reached from `initAuthInternal` / `signIn`" is false: `Settings.svelte`'s SYNC card calls `startAutoSync()` directly from its Auto-sync toggle, with no flag check and no `$isAuthenticated` check of its own. PR A adds the flag check inside `startAutoSync()` itself (one choke point for both the claimed callers and the toggle) rather than amending the false claim into a true one about a second call site.
2. **Guard count is 20, not 19.** Decision 3's named list of 19 commands is correct as far as it goes, but its own stated criterion ("every command... that touches session storage, the master key, or Supabase") also covers `test_encryption` (`src-tauri/src/sync/commands.rs`), which reads the master key via `keystore::load_master_key()` and was not in the original list. PR A guards all 20.
3. **`README.md` is removed from the "Code touched" list above.** #3196 (merged, same docs PR that wrote this ADR) already added the `## Account and sync (present, off by default)` section to `README.md`. The build scope needs zero README changes.
4. **`notifications.rs`'s private session pair is noted, not fixed, in #3195.** Gating the poller's start (correction 1) does not touch `notifications.rs`'s own private `load_auth_session` / `save_auth_session` functions, which read and write `~/Pane/.pane/auth_session.json` directly, independent of and bypassing the guarded `sync/commands.rs` functions of the same name. That duplication is real and pre-existing; folding it into one code path is tracked for later cleanup, not part of this ADR's build scope.

## Addendum B — Decision 5 correction: `push_notification` was a second, unguarded door (#3201 FAIL fix)

**Date:** 2026-11-10
**Status:** Factual correction only — nothing else in this ADR is reworded.

Reed's #3201 review (PR A of this split) found that correction 1 above — gating the poller's launch — closed only one of two paths into the `load_auth_session` → `refresh_auth_token` → `save_auth_session` triad. `notifications::push_notification`, a registered `#[tauri::command]` with zero frontend callers but fully reachable over IPC, calls `get_valid_token()` directly and carried no `require_account_enabled()` guard. This made Decision 5's safety rule false as written: a flag-off build *could* reach the poller's `save_auth_session` effect, through `push_notification`, even with the poller itself gated.

`push_notification` is now gated the same way (literal first statement, same `require_account_enabled()` function), and the three wrapper commands named in Decision 1 (`auth_email_check`, `auth_check_entitlement`, `auth_take_over_machine` — still removed outright in a later PR of this split, not kept behind the flag) are gated as an interim measure since they ship reachable in PR A before that later deletion lands. Decision 5's text above is corrected to name both guarded call sites explicitly, so the claim it makes is the claim the code now enforces, not an assumption that held only as far as the poller's own launch path.

## Addendum C — Decision 3 correction: "Account block" meant the whole tab (#3195 PR D)

**Date:** 2026-11-10
**Status:** Factual correction only — nothing else in this ADR is reworded.

Matt's live 1.0.85 flag-off walk found that `Settings.svelte`'s entire "Account & Sync" tab — the segmented-control tab button and the panel it opens, including the SYNC sub-block (Auto-sync toggle, Sync Now, status dot, Device Name, Sync History) — still rendered with the flag off. Decision 3's text above ("Settings renders its Account block only when on") is precise about the inner ACCOUNT sign-in sub-block, which PR B correctly gated, but silent about the tab shell and the SYNC sub-block beside it, which nothing gated. This directly contradicts Decision 2's stricter framing above ("No shipped UI surface reaches them while the flag is off") and is the sentence that licensed the gap.

PR D gates both tab render sites on the existing `accountEnabled` state: the segmented-control `<button>` is wrapped in `{#if accountEnabled}...{/if}`, and the panel's opening condition changes from `{#if activeTab === 'account'}` to `{#if activeTab === 'account' && accountEnabled}`. The pre-existing inner `{#if accountEnabled}` gate around just the ACCOUNT sub-block becomes redundant once the outer gate is in place but is left as-is, unrequested changes to `Settings.svelte` being out of scope for this correction. Decision 3's text is corrected to read "Settings renders its Account & Sync tab — tab button and panel both — only when on," matching both the actual fix and Decision 2's framing. The matching sentence in `docs/spec/onboarding.md` §6.0 receives the identical correction.
