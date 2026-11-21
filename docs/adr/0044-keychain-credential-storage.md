# ADR-0044: Keychain Credential Storage Contract

**Status:** Accepted
**Date:** 2026-06-17
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0024 (Local-only decryption — keychain as the trust anchor for sensitive material), ADR-0012 / ADR-0015 (gateway is the chat-layer auth target when `provider = openclaw`), ADR-0026 (install_uuid; backup keychain-export precedent)
**Related:** #1008 (this ADR's issue), #1167 (the `pane_gateway_token → api_key` collapse that created the shared-field conflation), #1498 (sync_key.bin — may reuse this contract, out of scope here), #1499 (#1167-migration — out of scope)

## Context

Three credentials live in plaintext in SQLite `ai_settings` and are mirrored unredacted to `~/Pane/.pane/config.json`:

- **Primary provider key** — `ai_settings.api_key` when `provider ∈ {openrouter, anthropic}` (the user's OpenRouter or Anthropic API key).
- **OC gateway token** — `ai_settings.api_key` when `provider = openclaw`. This is the SAME field. Since #1167 collapsed `pane_gateway_token` into `api_key`, the one `api_key` field means "provider key" or "gateway bearer token" depending on `provider`. Every reader must check `provider` to know what the value is.
- **Smart Features key** — `ai_settings.internal_api_key`, the §29 internal-provider key used for classification, rerank, and extraction (direct dispatch, never gateway-proxied).

The plaintext exposure is twofold: the SQLite row, and the `config.json` mirror written by `flush_ai_to_config()` (per file-system.md §2.3C, config.json is the source-of-truth settings store; the `ai_settings` table is its runtime cache). The §2.3C key-mapping table names the config.json field `apiKeyRef` — a name that implies a *reference*, not the secret itself — yet the current flush writes the actual secret there. The name records an intent the code never honored.

Supabase session tokens are already correctly keychain-resident (`supabase_access_token` / `supabase_refresh_token`, service `com.threelane.pane`, account-label-distinguished), per onboarding.md §6.1.5. That split — keychain holds the bearer, an on-disk metadata file holds non-secret routing fields — is the established precedent this ADR extends to AI credentials.

### What if we didn't decide

- **Three bearer-equivalent secrets stay in plaintext** in two on-disk locations (SQLite + config.json), readable by anything with filesystem access to the user's home directory. This is the exact threat ADR-0024 §1 names ("sensitive auth material is keychain-protected") and that the Supabase-auth audit flagged for session tokens — now applied to provider keys and the gateway token.
- **The `apiKeyRef` misnomer persists** — a config.json field named "reference" that holds the secret. A future reader trusts the name, assumes redaction is already in place, and builds on a false premise.
- **The shared-`api_key` conflation has no storage-layer resolution.** Whether or not the secret moves to keychain, the one-field-two-meanings problem makes every read site carry a provider-check. The keychain migration is the natural moment to split the storage so each credential has its own home.
- **Backups silently omit or mis-handle the new secrets** — `get_known_keychain_accounts()` enumerates a fixed account list for backup export; if AI credentials move to keychain without being added to that list, backups lose them, and machine-migration restore produces a non-functional AI config with no error.

## Decision

### 1. Three credentials, three keychain accounts, namespaced. The shared `api_key` field is split at the storage layer.

The three credentials move to the OS keychain (service `com.threelane.pane`, the existing service identifier) as three distinct accounts:

| Credential | Keychain account label | Replaces (plaintext today) |
|---|---|---|
| Primary provider key (OR / Anthropic) | `ai_provider_key` | `ai_settings.api_key` when `provider ∈ {openrouter, anthropic}` |
| OC gateway bearer token | `ai_gateway_token` | `ai_settings.api_key` when `provider = openclaw` |
| Smart Features (internal) key | `ai_internal_key` | `ai_settings.internal_api_key` |

**The shared-field conflation is resolved by splitting.** Today `ai_settings.api_key` holds the provider key OR the gateway token depending on `provider`, so every read site must branch on `provider` to interpret the value. Splitting into `ai_provider_key` and `ai_gateway_token` at the keychain layer means each read site fetches the credential it actually needs by name — the provider-check is no longer required to *interpret* the credential (it may still select which account to read, but the value's meaning is unambiguous once read). A gateway token can never be mistaken for a provider key, and vice versa.

**Naming convention.** The `ai_` prefix namespaces these against the existing `supabase_*` auth accounts and any future credential families, matching the established `{family}_{purpose}` account-label pattern (`supabase_access_token`, `supabase_refresh_token`). The three names are fixed string constants — not derived from `provider` or any runtime value — so the backup exporter (Decision 4) can enumerate them statically.

**Why not keep one shared keychain account.** A single `ai_api_key` keychain entry would carry the conflation into the keychain unchanged — readers would still branch on `provider` to interpret it, and the gateway token and provider key would still occupy one slot (so a user with both a primary OC connection and... no, only one primary at a time — but the internal key is always separate, and a future multi-provider config would re-collide). Splitting now, at the migration boundary, is cheaper than splitting later against keychain-resident data.

### 2. Migration contract — copy-then-clear, confirmed-write-before-delete, idempotent, flagged.

Migration runs at app launch (the same integrity-check phase pattern used elsewhere). For each of the three credentials:

**Ordering — copy then clear, never the reverse.**
1. Read the plaintext value from `ai_settings` (and/or config.json).
2. Write it to the keychain account (Decision 1).
3. **Confirm the keychain write succeeded** — read it back, verify the value matches.
4. ONLY after confirmed read-back: clear the plaintext from `ai_settings` and ensure the config.json mirror is redacted (Decision 3).

The DB plaintext is never cleared before the keychain write is confirmed. A keychain write that fails (locked keychain, permission denial, OS error) leaves the plaintext `ai_settings` value untouched and the migration aborts for that credential — to be retried on the next launch.

**Failure handling — abort, preserve, retry.** On any keychain write/read-back failure:
- Do NOT clear the `ai_settings` plaintext value.
- Do NOT set the migration-complete flag.
- Log the failure (without logging the secret value).
- The app continues to function using the plaintext value (the read sites fall back per the migration-aware read order below).
- The migration retries on the next launch.

**Migration-complete flag.** A per-credential (or single aggregate, implementation's choice) flag records that migration succeeded, so the migration pass is a no-op on subsequent launches. The flag's presence means "the keychain is the source of truth for this credential; `ai_settings` no longer holds it."

**Idempotency — enumerated states.** The migration pass, run every launch, handles all of:
- **State A — never migrated (plaintext present, no keychain entry, no flag):** the normal first-run-after-upgrade case. Copy → confirm → clear → set flag.
- **State B — fully migrated (keychain entry present, plaintext cleared, flag set):** no-op. The flag short-circuits the pass.
- **State C — partial / interrupted (keychain entry present from a prior attempt, plaintext NOT yet cleared, flag NOT set):** the prior launch wrote keychain but crashed before clearing DB. Re-confirm the keychain value (read-back), then clear the plaintext and set the flag. The keychain write is idempotent (overwrite-same-value is safe); do not assume the partial keychain value is correct without read-back against the still-present plaintext — if they diverge, the plaintext (the value the app has been running on) wins and is re-written to keychain.
- **State D — keychain present, plaintext absent, flag absent:** equivalent to State C with nothing left to clear; set the flag. (Distinguishes a crash between clear and flag-set.)
- **State E — no plaintext, no keychain, no flag (fresh install):** nothing to migrate. New credentials are written directly to keychain by the configuration write path (post-migration regime); set the flag so the pass no-ops thereafter. Fresh installs never touch the plaintext path.
- **State F — credential absent entirely (user never configured this provider):** no-op for that credential; the flag may still be set (nothing to migrate is a completed migration).

The flag is the fast-path; the state enumeration is the correctness guarantee when the flag is absent.

### 3. config.json redaction — secrets are NEVER written to config.json. Normative.

`flush_ai_to_config()` (and any other writer of config.json) MUST NOT write any of the three credential values to `~/Pane/.pane/config.json`. This is a normative rule, not an optimization.

- **`apiKeyRef` sentinel.** The config.json `ai` section's `apiKeyRef` field (per §2.3C key-mapping) holds a non-secret sentinel — a fixed marker (e.g. `"keychain"`) indicating the credential lives in the keychain, never the credential itself. The field name finally matches its content: a reference, not the secret.
- **aiSettings dump blocklist.** Any code path that serializes the `ai_settings` table (or its in-memory equivalent) into config.json MUST apply a blocklist that excludes the three credential keys (`api_key` / `internal_api_key` in their current forms, and any future-named credential key). The blocklist is the mechanical guard: even if a future field is added that holds a secret, the dump excludes anything on the list.
- **Recovery path implication.** §2.3C's `restore_ai_settings_from_config()` recovery path (DB empty → repopulate from config.json) can no longer recover credentials — they're not in config.json. After this ADR, credential recovery is from the keychain, not config.json. The recovery path repopulates non-secret config from config.json and reads credentials from the keychain; if the keychain lacks them, the credential is "not configured" (Decision 5).

### 4. Backup — the keychain export enumerates the new fixed accounts.

`get_known_keychain_accounts()` (the fixed account list that `backup.rs` exports for machine migration, per ADR-0026's keychain-export precedent) MUST include the three new accounts (`ai_provider_key`, `ai_gateway_token`, `ai_internal_key`).

Because the account labels are fixed string constants (Decision 1), the exporter enumerates them statically — no runtime derivation. The backup export blob remains plaintext on disk per the ADR-0024 backup-encryption amendment (2026-05-07): backup-file confidentiality at rest is the user's responsibility; the keychain export is decrypted-extraction for migration. The three AI credentials join the existing `supabase_*` accounts in that export.

Restore reseeds the keychain from the export. A backup created before this ADR (no AI accounts in the export) restores with no AI credentials in keychain — the post-restore state is "AI not configured" (Decision 5), recoverable via Settings re-entry. This is acceptable: a pre-ADR backup predates keychain-resident AI credentials, so there's nothing to lose that wasn't already going to require re-entry.

### 5. Post-migration read miss — "not configured," never a silent plaintext fallback.

Once the migration-complete flag is set (the post-migration regime), a keychain read miss for any of the three credentials returns "not configured" — the same state as a user who never entered the credential. It MUST NOT fall back to reading the plaintext `ai_settings` value.

The reasoning: after migration, `ai_settings` no longer holds the credential (it was cleared in Decision 2 step 4). A keychain miss means the credential is genuinely absent (keychain wiped, OS reinstall without backup restore, manual keychain deletion) — and the correct response is to prompt the user to re-enter it via Settings, not to silently revive a plaintext path that the migration deliberately emptied. A silent fallback would also mask keychain failures that the user needs to know about (a keychain that intermittently fails to unlock should surface as "re-authenticate," not silently degrade).

**Migration-aware read order (the only place plaintext is read post-decision):** during the migration window (flag NOT yet set), read sites read keychain-first, then fall back to plaintext `ai_settings` (the value migration hasn't moved yet). Once the flag IS set, read sites read keychain-only; a miss is "not configured." The flag is the switch between the two read regimes.

### 6. Rollback stance — downgrade yields non-functional AI; accepted PRE-LAUNCH ONLY.

A Pane version predating this ADR reads credentials from `ai_settings` plaintext. After migration clears that plaintext, a downgrade to a pre-ADR version finds no credential in `ai_settings` → AI is non-functional until the user re-enters credentials in the old version (which re-populates the plaintext path the old version understands).

**This "downgrade = non-functional AI, accepted" rationale holds ONLY pre-launch.** Pre-launch, the user base is the developer (Matt) on known hardware; a downgrade is a deliberate dev action, and re-entering a key is a known cost. **At launch, this MUST be revisited.** A shipped product cannot assume downgrades are rare developer actions — a customer who downgrades (or whose auto-update rolls back) must not silently lose AI functionality with no explanation. The launch-time options (not decided here — flagged for a launch-gate revisit): a migration that leaves a readable-but-deprecated plaintext copy for one version cycle; a downgrade-detection warning; or a hard floor on downgrade. **This ADR explicitly defers the launch-time rollback decision and marks it as a launch-blocking revisit.**

## Rationale

### Why keychain (not encrypted-in-DB)

ADR-0024 already establishes the keychain as the trust anchor for sensitive material, and Supabase tokens already live there. Encrypting credentials in SQLite would require a key, and that key would live... in the keychain — so the credential might as well live there directly. The keychain is the OS-provided secret store; using it for all bearer-equivalent secrets is the consistent stance.

### Why split the shared field (Decision 1)

The #1167 collapse merged `pane_gateway_token` into `api_key` for a good reason at the time (the chat and sync layers share one auth target when `provider = openclaw`, so one field served both). But that collapse made `api_key` polymorphic — its meaning depends on `provider`. Keychain migration is the cheapest moment to split, because we're touching every read site anyway. Splitting against keychain-resident data later would be a second migration.

### Why copy-then-clear with read-back confirmation (Decision 2)

The failure that must never happen: plaintext cleared, keychain write silently failed, credential lost. Read-back confirmation before clear is the guard. The cost (one extra keychain read per credential per migration, once) is trivial against the consequence (silent credential loss requiring re-entry with no error).

### Why "not configured" on post-migration miss (Decision 5)

A silent plaintext fallback would defeat the migration: the plaintext path would remain live, and the security benefit (no plaintext on disk) would be contingent on the keychain never missing. Treating a miss as "not configured" makes the keychain the single source of truth post-migration and surfaces keychain failures to the user instead of masking them.

### Why redaction is normative, not best-effort (Decision 3)

The `apiKeyRef` misnomer is evidence that a non-normative intent gets lost: the field was named for a reference, but the code wrote the secret. Making redaction normative (sentinel + blocklist) means a future field addition can't accidentally re-introduce a plaintext mirror — the blocklist is the mechanical enforcement that discipline alone failed to provide.

## Consequences

### Positive

- Three bearer-equivalent secrets leave plaintext on-disk storage entirely (SQLite + config.json), aligning AI credentials with the Supabase-token precedent and ADR-0024's trust model.
- The shared-`api_key` conflation is resolved — read sites fetch credentials by unambiguous name.
- The `apiKeyRef` field name finally matches its content (a reference/sentinel, not the secret).
- Backups correctly carry AI credentials for machine migration.
- A keychain failure surfaces as "re-authenticate" rather than silently degrading or reviving plaintext.

### Negative

- Migration touches every install once (the developer's, pre-launch). Cost is bounded: three credentials, one launch, idempotent.
- Every read site changes from "read `ai_settings.api_key`" to the migration-aware keychain-first read. Larger blast radius than a single-function change — but it's the read sites the conflation already forced to be provider-aware, so they're being touched anyway.
- Downgrade yields non-functional AI (Decision 6) — accepted pre-launch, explicitly flagged for launch revisit.

### Risks

- **Keychain unavailable at migration (locked, permission denied).** Mitigated by copy-then-clear: plaintext survives, migration retries next launch (State A persists until the keychain is writable).
- **Partial migration across a crash.** Mitigated by the State C/D handling (read-back reconciliation, plaintext wins on divergence).
- **Pre-ADR backup restored post-ADR.** Yields "AI not configured," recoverable via Settings. Acceptable (Decision 4).
- **A future credential added without updating the blocklist or the backup account list.** Mitigated partially by the blocklist being a named, central list; a CI check that the backup account list and the blocklist stay in sync with the credential set is a candidate follow-up (not gated by this ADR).
- **Downgrade post-launch.** The unmitigated risk Decision 6 defers — flagged as launch-blocking revisit.

## Alternatives Considered

**Encrypt credentials in SQLite with a keychain-held key.** Rejected — the key lives in the keychain anyway, so the credential might as well; adds an encryption layer that buys nothing over keychain-direct.

**Keep one shared `ai_api_key` keychain account (don't split).** Rejected — carries the #1167 conflation into the keychain unchanged; readers still branch on `provider` to interpret the value, and a future multi-provider config re-collides. Splitting at the migration boundary is cheaper than splitting later.

**Clear plaintext first, then write keychain (reverse ordering).** Rejected outright — a keychain write failure after the plaintext clear loses the credential silently. Copy-then-clear with read-back is the only safe ordering.

**Silent plaintext fallback on post-migration keychain miss.** Rejected (Decision 5 rationale) — defeats the migration's security benefit and masks keychain failures.

**Lazy migration (migrate on first read, not at launch).** Rejected — couples the read path to a state-writing side effect and hides migration timing from logs; the launch-phase integrity-check pattern keeps migration observable and the read path clean.

**Defer the whole thing post-launch.** Rejected — three plaintext bearer secrets on disk is a pre-launch security gap worth closing while the blast radius is one developer's install. (The rollback *sub*-decision, Decision 6, is the one piece correctly deferred to launch.)

## References

- **#1008** — this ADR's issue.
- **#1167** — the `pane_gateway_token → api_key` collapse that created the shared-field conflation. Migration precedent (copy-then-clear, spec-first).
- **ADR-0024** — Local-only decryption; keychain as trust anchor; backup-encryption amendment (plaintext export blob, user-responsibility-at-rest).
- **ADR-0012 / ADR-0015** — gateway is the chat-layer auth target when `provider = openclaw`; why chat and sync share one auth credential.
- **ADR-0026** — install_uuid; backup keychain-export precedent (`get_known_keychain_accounts`).
- **onboarding.md §6.1.5** — Supabase session-storage split (keychain bearer + on-disk metadata); the precedent this ADR extends.
- **file-system.md §2.3C** — config.json as settings source-of-truth, `ai_settings` as runtime cache, `flush_ai_to_config()` write path, `apiKeyRef` key mapping.
- **#1498 (sync_key.bin), #1499 (#1167-migration)** — out of scope; #1498 may reuse this contract later.

## Follow-ups (not gated by this ADR)

- **Launch-gate rollback revisit (Decision 6).** Launch-blocking. Decide the shipped-product downgrade behavior before launch.
- **CI check: blocklist ↔ backup-account-list ↔ credential-set sync.** Candidate mechanization so a future credential can't be added without updating both the config.json blocklist and the backup export list.
- **sync_key.bin (#1498) reuse of this contract.** Resolved — see Addendum A below.
- **`ai_internal_key`'s retirement + the plaintext `embeddings_api_key` row this ADR never accounted for.** Resolved — see Addendum B below.

## Addendum A — Sync master key (#1498)

**Date:** 2026-06-17
**Status:** Accepted (spec); NOT BUILT (#1498)

The sync master key (Argon2id-derived per ADR-0004; expands via BLAKE3-HKDF to the per-purpose keys encrypting all synced changesets, file blobs, and notifications) moves from `~/Pane/.pane/sync_key.bin` (base64, chmod 600) into the OS keychain, under this ADR's contract. Decisions 2–5 apply in full. The only difference from the AI credentials in the main ADR is that the migration *source* is a plaintext file, not a plaintext DB field.

**Keychain account.** Service `com.threelane.pane` (existing identifier), account label `sync_master_key` — a single fixed global label, not email-scoped. The value is the same base64-encoded master key that previously lived in `sync_key.bin`. This single fixed label assumes one Pane identity per machine (no email-scoping, unlike ADR-0044's AI accounts). That matches the prior single-file model and is the chosen behavior for now; multi-identity-per-machine, if it ever lands, would require revisiting the label scheme.

**Migration flag — DB, same as Decision 2.** Migration completion is recorded in the `migration_flags` table (`flag TEXT PRIMARY KEY, completed_at TEXT`), not a sentinel file. (`migration_flags` is preferred over `ai_settings` because the sync key is not an AI setting.) An earlier draft of this addendum proposed a sentinel file on the theory that `load_master_key()` is read before the DB opens; live source verification (`lib.rs:461` DB-open; consumers at `lib.rs:991` and `lib.rs:1014-1016` both receive `DbState`) confirmed every consumer runs after DB-open, so the DB flag is available at every read and the sentinel is unnecessary. The flag mechanism is therefore identical to Decision 2 — no divergence.

**Migration state machine (file source).** Per Decision 2's idempotent-state model, with "source present" reading as "`sync_key.bin` exists":
- **State A — not migrated:** `sync_key.bin` exists, no keychain entry, no flag. Read the base64 key from the file → write to keychain account `sync_master_key` → read back and confirm → delete `sync_key.bin` → set the `migration_flags` row. (Copy-then-clear; the file is never deleted before read-back confirms.)
- **State B — fully migrated:** keychain entry present, file absent, flag set. No-op.
- **State C — partial/interrupted:** keychain entry present, `sync_key.bin` still present, no flag. Re-confirm the keychain value by read-back against the still-present file; **on divergence, the plaintext file wins** (it is the value the app has been decrypting with) and is re-written to keychain. Then delete the file and set the flag.
- **State D — keychain present, file absent, no flag:** a crash between file-delete and flag-set. Set the flag.
- **State E — fresh install:** no file, no keychain entry, no flag. A newly derived master key is written directly to keychain; set the flag.
- **State F — flag set, `sync_key.bin` still present (asymmetric restore).** The flag is authoritative: migration already completed, so the file is a stale artifact, never a source. Delete it without reading.
  - *Keychain present* (file + keychain + flag): the keychain holds the live key; deleting the file just clears leftover plaintext. No read, no re-migration.
  - *Keychain absent* (file + no keychain + flag): the key is unavailable per Decision 5 (keychain wiped or not restored). The loose file is **not** trusted or re-adopted; delete it to remove the plaintext-on-disk hazard, and surface the read as key-unavailable / re-derive-or-restore. Contrast State A: a *pre-flag* legacy file has no flag and migrates normally — once the flag is set, a loose file is never re-adopted.

Once the flag is set, `sync_key.bin` is never read or re-adopted; if found it is deleted as stale. The copy-then-clear protection (never delete before a read-back-confirmed keychain copy) governs the pre-flag window only; post-flag, the keychain export is the sole supported key carrier.

**`load_master_key()` read order.** The accessor `keystore::load_master_key()` is the single point all four consumers read through (verified sole accessor; no consumer reads `sync_key.bin` directly). During the migration window (flag unset) it reads keychain-first, falling back to `sync_key.bin`. Once the flag is set it reads keychain-only; a miss is "key not available" (below). The read-order switch lives in the one accessor; it is not duplicated per call site.

**`delete_master_key()` contract.** Post-migration (flag set), `delete_master_key()` deletes the keychain entry, not a file (the file is already gone). It does not clear the migration flag — a deleted key on a migrated install is "migrated, key now absent," which routes to the not-available state rather than re-triggering file-based migration.

**Post-migration read miss (Decision 5).** Once the flag is set, a keychain miss for `sync_master_key` means the key is genuinely unavailable (keychain wiped, OS reinstall without backup restore, manual deletion). Per ADR-0004 the master key is unrecoverable without the user's passphrase or a backup, so a miss surfaces as the existing key-unavailable / re-derive-or-restore path — NOT a silent fallback to a `sync_key.bin` that migration deleted.

**Backup (Decision 4).** `get_known_keychain_accounts()` includes `sync_master_key` as a static entry, so the keychain export (machine-migration restore; plaintext export blob per the ADR-0024 backup-encryption amendment) carries the master key. Without it, a restored machine holds synced ciphertext it cannot decrypt. A backup created before #1498 carries no `sync_master_key` account; if its file portion captured the legacy `sync_key.bin`, restore routes that file through State A on next launch.

**Rollback (Decision 6).** Decision 6 applies equally to the sync master key. After migration clears `sync_key.bin` (State A), a downgrade to a pre-keychain build finds no key file — the old build reads only `sync_key.bin`, which is gone — so all synced content is undecryptable until the user upgrades back. This is more severe than the AI-credential rollback: a missing AI credential is recoverable by re-entering a key, whereas undecryptable synced notes cannot be recovered short of returning to a keychain-aware build. The failure is inherent to the migration design (Decision 2 clears the plaintext source; Decision 5 forbids any fallback to it) and is accepted PRE-LAUNCH ONLY, for the same reason as the AI-credential rollback — the user base is the developer on known hardware, and a downgrade is a deliberate act. The same launch-blocking revisit Decision 6 flags applies here: a shipped product cannot assume downgrades are rare developer actions, and the launch-time decision (hard version floor vs. downgrade-detection vs. one-cycle shadow) must cover the sync master key alongside the AI credentials. (#1519)

**Explicitly N/A (main-ADR AI-credential specifics, deliberately not applied):** config.json redaction (the sync key has no config.json mirror), the shared-field split (one key, one location, no conflation), and the `apiKeyRef` sentinel (that is the config.json marker for AI credentials; the sync key has no config.json presence).
- **TLS pin material (`gateway_tls_cert_pem` / `gateway_tls_fingerprint`).** Source shows these are plaintext `ai_settings` keys. Whether they're sensitive enough to join the keychain set is a separate scope question — flagged, not decided here.

## Addendum B — `ai_internal_key` retirement + the `embeddings_api_key` row this ADR never named (#3171 PR 1, C50)

**Date:** 2026-11-04
**Status:** Accepted; BUILT (#3171 PR 1, merged — `backfill_feature_key_into_connection`/`run_feature_key_consolidation`, `credential_migration.rs`)

Reed's #3172 audit (§6) of #3171 PR 1 found that PR correctly and completely updated the DERIVED spec prose (`docs/spec/ai-integration.md`'s credential-storage summary line, `docs/spec/memory-pipeline.md` §5.9.2) but left this ADR document itself untouched — Decision 1's own table and Decision 4's backup-accounts prose still describe `ai_internal_key` as one of this ADR's three permanent, going-forward keychain accounts, and `embeddings_api_key` was never named anywhere in this ADR at all (confirmed via `grep`, zero hits, prior to this Addendum). This addendum records what PR 1 actually built, mirroring Addendum A's precedent of documenting a contract extension here rather than leaving the main Decision sections silently superseded by out-of-document prose.

**What changed, relative to Decision 1's table.** Decision 1 named three permanent accounts: `ai_provider_key` (primary OR/Anthropic key — itself already demoted to migration-only by the later #2565 provider-split, B3, which this ADR's own text predates and does not reflect either), `ai_gateway_token`, and `ai_internal_key` (Smart Features' key). #3171 PR 1 adds a fourth, functionally equivalent demotion: **`ai_internal_key` is now migration-only**, exactly like `ai_provider_key` already is. Smart Features and Semantic Search no longer have (or ask for) their own stored key at all — both read the SAME per-provider Provider Connections keychain accounts every other AI feature already resolves credentials from (`ai_openrouter_key` / `ai_anthropic_key` / `ai_venice_key`, via `credential_migration::read_provider_key`), selected per-feature by that feature's own provider dropdown (`internalProvider` / `ssProvider` — the per-feature PROVIDER CHOICE is unchanged and still legitimately independent per feature, per `memory-pipeline.md` §5.9.2's own worked example; only the per-feature KEY FIELD is gone).

**The plaintext row this ADR never accounted for.** `ai_settings.embeddings_api_key` (Semantic Search's own key, used only when the embeddings provider could take one — OpenRouter) was PLAINTEXT, had no keychain account at all, and was never one of Decision 1's three named credentials — a gap in this ADR's own original scope, not a regression introduced later. #3171 PR 1 retires it the same way: no longer read, no longer persisted; its resolution now goes through the same `read_provider_key(&conn, "openrouter")` path as everything else. Venice's embeddings arm was already structured this way before PR 1 (`memory-pipeline.md` §5.9.2 names it "the one deliberate exception") — PR 1 makes OpenRouter's arm (and both of Smart Features' key-based arms) match Venice's existing shape, rather than inventing a new one.

**The mechanism — Decision 2's shape, applied to a NEW migration target.** A one-time, idempotent, flagged pass (`feature_key_consolidation_v1`, `run_feature_key_consolidation`) runs the exact copy-then-clear, confirmed-write-before-delete discipline Decision 2 already specifies, for each of the two retired slots: if the matching Provider Connections account is empty, backfill the feature's own value into it, read back to confirm, then clear the feature-level copy (`ai_internal_key` deleted from keychain; `embeddings_api_key` row deleted from `ai_settings`); if the connection already holds a USABLE key (see the fast-follow immediately below), discard the feature-level copy instead (logged, never silent) without ever writing it anywhere. Global flag, AND-gated on both Smart Features and Semantic Search succeeding in the same pass — a partial failure retries both halves next launch; the already-succeeded half is a true no-op on retry (verified by Reed's audit §1, run live against an injected partial-failure-then-retry scenario, not just read from the `&&`).

**Fast-follow, same commit as this Addendum: the discard path's missing emptiness/whitespace guard.** Reed's audit §4 found `backfill_feature_key_into_connection`'s discard branch originally fired on `connection_key.is_some()` alone — a `Some("")` or whitespace-only Provider Connections value would "win" over a feature's real key, discarding it. Traced as unreachable via any LIVE write path at the time (every caller that can write `ai_openrouter_key`/`ai_anthropic_key`/`ai_venice_key` either rejects `None` outright or is gated by a UI that trims-and-disables on blank input), but a genuine missing local invariant in code specifically about never losing a user-typed credential — closed here: the discard branch now checks `connection_key.as_deref().is_some_and(|v| !v.trim().is_empty())`, treating an empty/whitespace connection value identically to "no connection key yet" (falls through to backfill, overwriting the blank).

**Decision 4 (backup) — mechanism unaffected, framing stale.** `ai_internal_key` stays in `ALL_AI_KEYCHAIN_ACCOUNTS` (`credential_migration.rs`) — the SAME migration-only-but-still-enumerated treatment `ai_provider_key` already has, so a backup taken mid-migration-window still exports whatever value remains in that account. Decision 4's prose ("MUST include the three new accounts... `ai_internal_key`") is now stale FRAMING — the account is enumerated for migration-window completeness, not because it is still a going-forward credential — but the underlying mechanism needed no code change.

**Not applicable to `embeddings_api_key`.** Unlike `ai_internal_key`, `embeddings_api_key` was never a keychain account (Decision 1/4 are keychain-only decisions) — it was a plaintext `ai_settings` row this ADR's scope never covered. Its retirement is recorded here for completeness (so a reader searching this ADR for "embeddings_api_key" finds it) but does not amend Decision 1's table or Decision 4's account list — there is nothing in either to amend for a row that was never a keychain account.

**Rollback (Decision 6).** Unaffected in kind — a downgrade to a pre-#3171-PR-1 build finds no `embeddings_api_key` row and no `ai_internal_key` keychain entry (both cleared by the migration), so the old build's Smart Features/Semantic Search sections read as "not configured" until re-entered in that old build's own UI. Same accepted-pre-launch-only posture as the rest of Decision 6; no new rollback hazard introduced (both retired slots are strictly OPTIONAL per-feature conveniences, not the primary/gateway credentials Decision 6's own worked example concerns).

**Spec cross-references (already correct, unaffected by this Addendum):** `docs/spec/ai-integration.md`'s credential-storage summary line (demotes `ai_internal_key`, names `embeddings_api_key`'s retirement for the first time) and the Smart Features section (`internal_configured_at`/probe-skip mechanics); `docs/spec/memory-pipeline.md` §5.9.2 (Credential source paragraph + "Venice is the one deliberate exception" paragraph, Venice's shape now the norm for all three engines); `docs/spec/runtime.md` §39.1 (closes its own forward reference to this ticket). This Addendum amends the ADR DOCUMENT itself to match what those spec sections and the shipped code already state — no behavior changes as a result of writing this Addendum.
