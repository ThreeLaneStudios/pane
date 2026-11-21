// ai/credential_migration.rs — ADR-0044: Keychain Credential Storage (§1008)
// Built: 2026-06-17 (v0.67.12, issue #1008)
//
// Three AI credentials leave plaintext SQLite/config.json and move to OS keychain.
// Migration is idempotent, flagged, and handles six enumerated states (A–F).
// Post-migration: keychain miss = "not configured." No plaintext fallback ever.
//
// Decision 1: Three accounts, fixed names, service = "com.threelane.pane"
// Decision 2: Copy-then-clear with read-back confirmation
// Decision 5: Migration-aware read order; post-flag = keychain only

use rusqlite::Connection;

// ─── Test-only log capture (C5 #2584) ──────────────────────────────────────
//
// `eprintln!` output is swallowed by Rust's default test harness before it
// reaches the OS-level stderr file descriptor (libtest intercepts at the
// io::Write layer per-test, not the fd layer), so fd-level capture crates
// can't reliably observe it from inside a `#[test]` without `--nocapture`.
// `log_credential!` below always calls `eprintln!` (unchanged production
// behavior/format) and, only in test builds, also appends the formatted
// message to a thread-local buffer so tests can assert on exact log content.
// Each `#[test]` runs on its own thread under the default libtest runner, so
// this is naturally test-isolated without needing a mutex.
#[cfg(test)]
thread_local! {
    static TEST_LOG_CAPTURE: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
}

#[cfg(test)]
pub(crate) fn test_capture_log(msg: &str) {
    TEST_LOG_CAPTURE.with(|c| c.borrow_mut().push(msg.to_string()));
}

/// Test helper: drain and return everything captured on this thread so far.
#[cfg(test)]
pub(crate) fn test_take_captured_logs() -> Vec<String> {
    TEST_LOG_CAPTURE.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

macro_rules! log_credential {
    ($($arg:tt)*) => {{
        let __msg = format!($($arg)*);
        eprintln!("{}", __msg);
        #[cfg(test)]
        crate::ai::credential_migration::test_capture_log(&__msg);
    }};
}

// ─── Keychain Account Constants (Decision 1) ───────────────────────────────

/// B3 (#2565): Legacy shared account for the primary provider key
/// (OpenRouter / Anthropic). Read-only during migration, do not use for new
/// writes — anthropic and openrouter each have their own account now (see
/// `KEYCHAIN_ACCOUNT_ANTHROPIC_KEY` / `KEYCHAIN_ACCOUNT_OPENROUTER_KEY`
/// below). Kept available for migration and backup because an ambiguous legacy
/// value is preserved rather than deleted when its provider cannot be inferred.
#[allow(dead_code)]
pub const KEYCHAIN_ACCOUNT_PROVIDER_KEY: &str = "ai_provider_key";

/// Keychain account for the Anthropic API key. Split from the legacy shared
/// `ai_provider_key` slot by B3 (#2565) — see `migrate_v82`.
pub const KEYCHAIN_ACCOUNT_ANTHROPIC_KEY: &str = "ai_anthropic_key";

/// Keychain account for the OpenRouter API key. Split from the legacy shared
/// `ai_provider_key` slot by B3 (#2565) — see `migrate_v82`.
pub const KEYCHAIN_ACCOUNT_OPENROUTER_KEY: &str = "ai_openrouter_key";

/// Keychain account for the OC gateway bearer token.
/// Replaces `ai_settings.api_key` when `provider = openclaw`.
pub const KEYCHAIN_ACCOUNT_GATEWAY_TOKEN: &str = "ai_gateway_token";

/// Keychain account for the Smart Features (internal) provider key.
/// Replaces `ai_settings.internal_api_key`.
pub const KEYCHAIN_ACCOUNT_INTERNAL_KEY: &str = "ai_internal_key";

/// Keychain account for the Brave Search API key (§4.6.8, Part 2, #2591,
/// ADR-0064 D5). Postdates ADR-0044 entirely — there is no legacy plaintext
/// slot for this key, unlike every account above it, so `read_brave_key` has
/// no migration-flag branch (see its doc comment below).
pub const KEYCHAIN_ACCOUNT_BRAVE_KEY: &str = "ai_brave_key";

/// ADR-0065 D4: Venice API key keychain account. Keychain-only — no legacy
/// plaintext fallback (same posture as `KEYCHAIN_ACCOUNT_BRAVE_KEY` / `read_brave_key`).
pub const KEYCHAIN_ACCOUNT_VENICE_KEY: &str = "ai_venice_key";

/// All AI credential account names — used by backup exporter (Decision 4).
/// The legacy account remains included because v82 deliberately preserves an
/// ambiguous value rather than risking destructive provider inference.
pub const ALL_AI_KEYCHAIN_ACCOUNTS: &[&str] = &[
    KEYCHAIN_ACCOUNT_PROVIDER_KEY,
    KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
    KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
    KEYCHAIN_ACCOUNT_GATEWAY_TOKEN,
    KEYCHAIN_ACCOUNT_INTERNAL_KEY,
    KEYCHAIN_ACCOUNT_BRAVE_KEY,
    KEYCHAIN_ACCOUNT_VENICE_KEY,
];

/// B3 (#2565): map a key-based `runtime_type` to its dedicated keychain
/// account. Returns `None` for runtime types that don't use a keychain API
/// key (`ollama`, `openclaw` — the latter uses `KEYCHAIN_ACCOUNT_GATEWAY_TOKEN`
/// via a separate read/write path) or anything unrecognized.
fn provider_keychain_account(runtime_type: &str) -> Option<&'static str> {
    match runtime_type {
        "anthropic" => Some(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY),
        "openrouter" => Some(KEYCHAIN_ACCOUNT_OPENROUTER_KEY),
        // ADR-0065 D4: Venice key is keychain-only (no legacy plaintext slot).
        "venice" => Some(KEYCHAIN_ACCOUNT_VENICE_KEY),
        _ => None,
    }
}

/// ai_settings key that records migration completion.
/// Value = "complete" when all three credentials have been migrated.
const MIGRATION_FLAG_KEY: &str = "keychain_migration_v1";
const MIGRATION_FLAG_COMPLETE: &str = "complete";

// ─── Migration-Flag Helpers ────────────────────────────────────────────────

fn is_migration_complete(conn: &Connection) -> bool {
    crate::database::get_ai_setting(conn, MIGRATION_FLAG_KEY)
        .map(|v| v == MIGRATION_FLAG_COMPLETE)
        .unwrap_or(false)
}

fn set_migration_complete(conn: &Connection) {
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE],
    ) {
        eprintln!("[CREDENTIAL-MIGRATION] Failed to set migration flag: {}", e);
    } else {
        eprintln!("[CREDENTIAL-MIGRATION] Migration flag set — post-migration regime active");
    }
}

// ─── Single-Credential Migration ──────────────────────────────────────────

/// Migrate one credential from plaintext ai_settings to keychain.
///
/// Handles States A–F per ADR-0044 Decision 2.
///
/// `db_key` — the ai_settings key holding the plaintext value.
/// `keychain_account` — the keychain account to write to.
/// `label` — human-readable name for log messages.
///
/// Returns true if migration succeeded (or was not needed), false on keychain failure.
fn migrate_one_credential(
    conn: &Connection,
    db_key: &str,
    keychain_account: &str,
    label: &str,
) -> bool {
    let plaintext = crate::database::get_ai_setting(conn, db_key);
    let keychain_val = match crate::keychain::get_password(keychain_account) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[CREDENTIAL-MIGRATION] Cannot read keychain for {}: {} — skipping",
                label, e
            );
            return false;
        }
    };

    match (plaintext.as_deref(), keychain_val.as_deref()) {
        // State F — credential never configured on either side
        (None, None) => {
            eprintln!(
                "[CREDENTIAL-MIGRATION] {} — State F (never configured), nothing to migrate",
                label
            );
            true
        }

        // State B/D — no plaintext, keychain present (or flag scenario)
        // State D: keychain present, plaintext absent, flag absent → set flag only
        (None, Some(_)) => {
            eprintln!(
                "[CREDENTIAL-MIGRATION] {} — State D/B (keychain present, no plaintext) — flag only",
                label
            );
            true
        }

        // State A — plaintext present, no keychain entry yet
        // State C — keychain present but plaintext NOT cleared (partial prior attempt)
        (Some(plain), keychain_opt) => {
            let effective_value = match keychain_opt {
                None => {
                    // State A — normal upgrade path
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — State A (plaintext present, no keychain) — migrating",
                        label
                    );
                    plain
                }
                Some(existing_kc) if existing_kc == plain => {
                    // State C — same value in both; still need to clear plaintext + set flag
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — State C (values match) — clearing plaintext",
                        label
                    );
                    plain
                }
                Some(_) => {
                    // State C diverge — plaintext wins (Decision 2: re-write to keychain)
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — State C (DIVERGE) — plaintext wins, re-writing keychain",
                        label
                    );
                    plain
                }
            };

            // Step 1: Write to keychain
            if let Err(e) = crate::keychain::store_password(keychain_account, effective_value) {
                eprintln!(
                    "[CREDENTIAL-MIGRATION] {} — keychain write failed: {} — preserving plaintext, will retry",
                    label, e
                );
                return false;
            }

            // Step 2: Read back to confirm (Decision 2 — confirmed-write-before-delete)
            match crate::keychain::get_password(keychain_account) {
                Ok(Some(ref confirmed)) if confirmed == effective_value => {
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — keychain read-back confirmed",
                        label
                    );
                }
                Ok(Some(_)) => {
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — keychain read-back VALUE MISMATCH — aborting, preserving plaintext",
                        label
                    );
                    return false;
                }
                Ok(None) => {
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — keychain read-back returned None — aborting, preserving plaintext",
                        label
                    );
                    return false;
                }
                Err(e) => {
                    eprintln!(
                        "[CREDENTIAL-MIGRATION] {} — keychain read-back failed: {} — preserving plaintext",
                        label, e
                    );
                    return false;
                }
            }

            // Step 3: Clear plaintext from ai_settings (ONLY after confirmed read-back)
            if let Err(e) = conn.execute(
                "DELETE FROM ai_settings WHERE key = ?1",
                rusqlite::params![db_key],
            ) {
                eprintln!(
                    "[CREDENTIAL-MIGRATION] {} — failed to clear plaintext from ai_settings: {} — keychain intact",
                    label, e
                );
                // Non-fatal: keychain has the value; plaintext will be handled on next launch
            } else {
                eprintln!(
                    "[CREDENTIAL-MIGRATION] {} — plaintext cleared from ai_settings",
                    label
                );
            }

            true
        }
    }
}

// ─── Public: Run Migration at Launch ──────────────────────────────────────

/// Run the credential migration pass. Called at app launch (integrity-check phase).
///
/// Idempotent — safe to call every launch. Fast-paths when flag is set (State B).
/// Handles States A–F per ADR-0044 Decision 2.
/// On keychain failure: preserves plaintext, skips flag, retries next launch.
pub fn run_credential_migration(conn: &Connection) {
    eprintln!("[CREDENTIAL-MIGRATION] Starting credential migration pass...");

    // Fast-path: flag set → State B, no-op
    if is_migration_complete(conn) {
        eprintln!("[CREDENTIAL-MIGRATION] Migration flag set — skipping (already migrated)");
        return;
    }

    eprintln!("[CREDENTIAL-MIGRATION] Migration flag absent — evaluating all three credentials");

    // Determine current provider so we know which keychain account api_key maps to
    let provider = crate::database::get_ai_setting(conn, "provider")
        .unwrap_or_default();

    // Migrate api_key → provider-appropriate account (B3, #2565: anthropic and
    // openrouter each route to their OWN account now — never the legacy shared
    // slot, which this launch-time migration no longer writes to at all).
    // The shared api_key field holds either the provider key or the gateway token
    // depending on provider. We split it here into the correct keychain account.
    let api_key_ok = match provider.as_str() {
        "openclaw" => migrate_one_credential(
            conn,
            "api_key",
            KEYCHAIN_ACCOUNT_GATEWAY_TOKEN,
            "gateway_token (from api_key)",
        ),
        "anthropic" => migrate_one_credential(
            conn,
            "api_key",
            KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
            "anthropic_key (from api_key)",
        ),
        "openrouter" => migrate_one_credential(
            conn,
            "api_key",
            KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
            "openrouter_key (from api_key)",
        ),
        other => {
            // Unrecognized/empty provider (e.g. ollama, or no provider configured
            // yet) — nothing to migrate for this credential. Not an error: treat
            // as a no-op success so it doesn't block the internal_key migration
            // or the overall flag from being set.
            eprintln!(
                "[CREDENTIAL-MIGRATION] api_key — provider '{}' is not key-based, skipping",
                other
            );
            true
        }
    };

    // Migrate internal_api_key → ai_internal_key
    let internal_key_ok = migrate_one_credential(
        conn,
        "internal_api_key",
        KEYCHAIN_ACCOUNT_INTERNAL_KEY,
        "internal_key",
    );

    // Set migration flag only if ALL credential migrations succeeded
    if api_key_ok && internal_key_ok {
        set_migration_complete(conn);
        eprintln!("[CREDENTIAL-MIGRATION] All credentials migrated — post-migration regime active");
    } else {
        eprintln!(
            "[CREDENTIAL-MIGRATION] One or more credentials failed to migrate — \
             will retry on next launch (api_key_ok={}, internal_key_ok={})",
            api_key_ok, internal_key_ok
        );
    }
}

// ─── C50 (#3171 PR 1): Feature-Level Key Consolidation ─────────────────────
//
// Three credential copies existed for provider keys: Provider Connections
// (keychain, per-provider — already correct, this module's own Decision 1
// split), Smart Features (`ai_internal_key`, one opaque keychain slot SHARED
// across openrouter/anthropic — the exact conflation this module's Decision 1
// fixed for the primary provider, left unfixed for this one feature), and
// Semantic Search (`ai_settings.embeddings_api_key`, PLAINTEXT — never in the
// keychain, never one of this module's three named accounts).
//
// Matt's migration rule (2026-11-04, issue #3171 comment): "Backfill the
// connection if absent, else discard; copy-confirm-clear." If a feature holds
// a key and no matching Provider Connection exists yet, backfill the
// connection from it — write, read back to CONFIRM, then clear the feature's
// own copy. Never clear before the confirmed write-back (same discipline as
// `migrate_one_credential`'s Decision 2 above). If a connection already
// exists, the feature-level copy is discarded (logged, not silently dropped)
// — the connection wins either way, so the feature's copy is ALWAYS gone by
// the end of a successful pass, not merely unread.
//
// Venice needs no entry here: Smart Features' Venice arm already writes
// straight into `KEYCHAIN_ACCOUNT_VENICE_KEY` (`save_venice_key`, called by
// `Settings.svelte`/`Onboarding.svelte`), never into `ai_internal_key` — and
// Semantic Search's Venice arm reads the chat-provider key directly
// (`read_venice_key`), never persisting into `embeddings_api_key`. Ollama
// needs no entry either — it has no key, only an endpoint, which this issue
// does not touch.
//
// Idempotent, flagged (mirrors `run_credential_migration`'s own flag shape
// immediately above) — a second launch after a successful pass is a no-op.
const FEATURE_KEY_CONSOLIDATION_FLAG_KEY: &str = "feature_key_consolidation_v1";
const FEATURE_KEY_CONSOLIDATION_COMPLETE: &str = "complete";

fn is_feature_key_consolidation_complete(conn: &Connection) -> bool {
    crate::database::get_ai_setting(conn, FEATURE_KEY_CONSOLIDATION_FLAG_KEY)
        .map(|v| v == FEATURE_KEY_CONSOLIDATION_COMPLETE)
        .unwrap_or(false)
}

fn set_feature_key_consolidation_complete(conn: &Connection) {
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![FEATURE_KEY_CONSOLIDATION_FLAG_KEY, FEATURE_KEY_CONSOLIDATION_COMPLETE],
    ) {
        eprintln!("[FEATURE-KEY-CONSOLIDATION] Failed to set flag: {}", e);
    } else {
        eprintln!("[FEATURE-KEY-CONSOLIDATION] Flag set — post-consolidation regime active");
    }
}

/// Backfill-if-absent-else-discard for ONE feature-level key into its matching
/// Provider Connections keychain slot.
///
/// `feature_value` is the current value of the feature's own key, already
/// resolved by the caller (e.g. `read_internal_key`'s keychain-then-plaintext
/// read for Smart Features, or a direct `ai_settings` plaintext read for
/// Semantic Search). `runtime_type` must be a key-based provider (`openrouter`
/// | `anthropic` | `venice`) — the caller maps the feature's own provider
/// selection to this value before calling in.
///
/// Returns `true` once the feature-level copy is confirmed GONE (backfilled-
/// then-cleared, discarded-then-cleared, or never present at all — a true
/// no-op). Returns `false` only when a write/read-back/clear step failed,
/// in which case the feature-level copy is deliberately PRESERVED for a retry
/// on the next launch — the confirmed-write-before-clear discipline means a
/// keychain failure must never destroy the only copy of the credential.
///
/// `clear_feature_copy` runs ONLY after a confirmed success path (either
/// branch) — never before.
fn backfill_feature_key_into_connection<F>(
    feature_value: Option<&str>,
    runtime_type: &str,
    label: &str,
    clear_feature_copy: F,
) -> bool
where
    F: FnOnce() -> Result<(), String>,
{
    let Some(value) = feature_value else {
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] {} — no feature-level key present, nothing to migrate",
            label
        );
        return true;
    };

    let Some(account) = provider_keychain_account(runtime_type) else {
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] {} — '{}' is not a key-based provider, cannot migrate — \
             preserving feature-level copy",
            label, runtime_type
        );
        return false;
    };

    // Does a Provider Connection already have a key for this provider?
    let connection_key = match crate::keychain::get_password(account) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[FEATURE-KEY-CONSOLIDATION] {} — cannot read connection keychain slot for '{}': {} — \
                 preserving feature-level copy, will retry",
                label, runtime_type, e
            );
            return false;
        }
    };

    // #3171 PR 2 fast-follow (Reed's #3172 audit §4): the discard branch
    // originally fired on `connection_key.is_some()` alone — no emptiness/
    // whitespace check. Unreachable via any LIVE write path today (every
    // caller that can write `ai_openrouter_key`/`ai_anthropic_key`/
    // `ai_venice_key` either rejects `None` outright or is gated by a UI
    // that trims-and-disables on blank input — see the audit's own
    // reachability trace), but this is a brand-new function specifically
    // about never losing a user-typed credential, so the local invariant
    // belongs here regardless of what today's callers happen to allow.
    // `Some("")`/whitespace-only is treated identically to `None` — "no
    // connection key yet" — falling through to the backfill path below
    // instead of discarding the feature's real key in favor of a blank
    // connection slot.
    let connection_key_usable = connection_key
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());

    if connection_key_usable {
        // Connection wins — discard the feature-level copy (logged, never silent).
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] {} — Provider Connections already has a '{}' key — \
             discarding the feature-level copy",
            label, runtime_type
        );
        return match clear_feature_copy() {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "[FEATURE-KEY-CONSOLIDATION] {} — failed to clear feature-level copy after discard: {} — \
                     will retry next launch",
                    label, e
                );
                false
            }
        };
    }

    // No USABLE connection key yet (absent, or present but empty/
    // whitespace-only) — backfill from the feature's own value.
    eprintln!(
        "[FEATURE-KEY-CONSOLIDATION] {} — no '{}' connection key yet — backfilling from the feature's own key",
        label, runtime_type
    );
    if let Err(e) = crate::keychain::store_password(account, value) {
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] {} — backfill write failed: {} — preserving feature-level copy",
            label, e
        );
        return false;
    }

    // Read back to CONFIRM (Decision 2 shape) before touching the feature's own copy.
    match crate::keychain::get_password(account) {
        Ok(Some(ref confirmed)) if confirmed == value => {
            eprintln!("[FEATURE-KEY-CONSOLIDATION] {} — backfill read-back confirmed", label);
        }
        other => {
            eprintln!(
                "[FEATURE-KEY-CONSOLIDATION] {} — backfill read-back MISMATCH/failure ({:?}) — \
                 aborting, preserving feature-level copy",
                label, other
            );
            return false;
        }
    }

    match clear_feature_copy() {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "[FEATURE-KEY-CONSOLIDATION] {} — failed to clear feature-level copy after backfill: {} — \
                 will retry next launch",
                label, e
            );
            false
        }
    }
}

/// Run the C50 feature-key consolidation pass. Idempotent — fast-paths once
/// the flag is set. Call AFTER `run_credential_migration` at launch (needs
/// `read_internal_key`'s keychain-aware read to already be routing
/// correctly for a pre-ADR-0044 install that hasn't run that migration yet
/// this launch).
pub fn run_feature_key_consolidation(conn: &Connection) {
    if is_feature_key_consolidation_complete(conn) {
        eprintln!("[FEATURE-KEY-CONSOLIDATION] Flag set — skipping (already consolidated)");
        return;
    }

    eprintln!("[FEATURE-KEY-CONSOLIDATION] Flag absent — evaluating Smart Features + Semantic Search");

    // Smart Features: `ai_internal_key` (read via `read_internal_key`'s
    // keychain-then-plaintext-fallback) only ever holds an openrouter or
    // anthropic key — see this section's doc comment above for why Venice
    // never reaches this slot. The provider it belongs to is whatever
    // `internal_provider` currently names (written in lockstep with the key
    // by `Settings.svelte`'s `saveInternalProvider` / `Onboarding.svelte`'s
    // `sfTestConnection`, so the two never diverge).
    let internal_provider =
        crate::database::get_ai_setting(conn, "internal_provider").unwrap_or_default();
    let smart_features_ok = if internal_provider == "openrouter" || internal_provider == "anthropic" {
        let feature_value = read_internal_key(conn);
        backfill_feature_key_into_connection(
            feature_value.as_deref(),
            &internal_provider,
            "smart_features_internal_key",
            || {
                // Clear BOTH possible locations: the keychain account (the
                // post-ADR-0044 state) and any plaintext leftover (an install
                // that hasn't run `run_credential_migration` yet this launch).
                // `delete_password` already treats "account not found" as Ok
                // (see keychain.rs), so this is safe to call unconditionally.
                crate::keychain::delete_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY)
                    .map_err(|e| format!("keychain delete of {} failed: {}", KEYCHAIN_ACCOUNT_INTERNAL_KEY, e))?;
                conn.execute("DELETE FROM ai_settings WHERE key = 'internal_api_key'", [])
                    .map_err(|e| format!("DELETE internal_api_key failed: {}", e))?;
                Ok(())
            },
        )
    } else {
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] smart_features_internal_key — internal_provider is '{}', \
             not openrouter/anthropic — nothing to migrate",
            internal_provider
        );
        true
    };

    // Semantic Search: `ai_settings.embeddings_api_key` is PLAINTEXT and is
    // unconditionally an OpenRouter key — see this section's doc comment
    // above for why Ollama (endpoint, not a key) and Venice (reads the
    // chat-provider key directly) never reach this slot.
    let embeddings_key_plaintext = crate::database::get_ai_setting(conn, "embeddings_api_key");
    let semantic_search_ok = backfill_feature_key_into_connection(
        embeddings_key_plaintext.as_deref(),
        "openrouter",
        "semantic_search_embeddings_api_key",
        || {
            conn.execute("DELETE FROM ai_settings WHERE key = 'embeddings_api_key'", [])
                .map_err(|e| format!("DELETE embeddings_api_key failed: {}", e))?;
            Ok(())
        },
    );

    if smart_features_ok && semantic_search_ok {
        set_feature_key_consolidation_complete(conn);
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] Complete — Smart Features + Semantic Search now read Provider Connections"
        );
    } else {
        eprintln!(
            "[FEATURE-KEY-CONSOLIDATION] One or more migrations incomplete — will retry next launch \
             (smart_features_ok={}, semantic_search_ok={})",
            smart_features_ok, semantic_search_ok
        );
    }
}

// ─── D26 (#2673 PR-A): DB-phase/keychain-phase snapshot ────────────────────
//
// `read_provider_key`/`read_gateway_token` below need two DB-derived inputs
// (the migration-complete flag, and the plaintext `api_key` fallback) plus one
// keychain read. Historically both lived behind a single `&Connection`
// parameter, which is exactly what forced every caller of these functions to
// keep `db.0` locked for the full duration of the keychain syscall too (see
// #2673 — a blocked macOS keychain authorization prompt held the lock
// indefinitely and silently stalled `ai_chat_stream`).
//
// `CredentialDbSnapshot` captures the two DB-derived inputs once, while the
// lock is still held, so callers can drop the lock and then run the actual
// keychain read (optionally inside `tokio::task::spawn_blocking` +
// `tokio::time::timeout`, mirroring the #1347 precedent at
// `sync/commands.rs:324-333`) with no live `&Connection` in scope. See
// docs/spec/ai-integration.md §28.4 for the full contract.
//
// `read_provider_key`/`read_gateway_token` remain as-is (still take `&Connection`
// directly) for every existing synchronous caller/test — they now just capture a
// snapshot internally and delegate to the `_from_snapshot` variants below, so
// there is exactly one copy of the actual read-order logic.
#[derive(Debug, Clone)]
pub struct CredentialDbSnapshot {
    migration_complete: bool,
    plaintext_api_key: Option<String>,
}

impl CredentialDbSnapshot {
    /// Capture the DB-derived inputs `read_provider_key`/`read_gateway_token` need.
    /// Call this while `db.0` is still locked; the returned snapshot has no
    /// connection borrow and can safely outlive the lock.
    pub fn capture(conn: &Connection) -> Self {
        Self {
            migration_complete: is_migration_complete(conn),
            plaintext_api_key: crate::database::get_ai_setting(conn, "api_key"),
        }
    }
}

// ─── Public: Migration-Aware Credential Reads (Decision 5) ────────────────

/// Read a per-provider API key (B3, #2565: Anthropic / OpenRouter each have
/// their own keychain slot — see `provider_keychain_account`).
///
/// `runtime_type` must be `"anthropic"` or `"openrouter"`; any other value
/// returns `None` immediately (these are the only two key-based providers).
///
/// Migration-aware read order (Decision 5, extended by B3):
/// - Flag NOT set → this provider's own keychain slot first, then fall back
///   to the legacy plaintext `ai_settings.api_key`. The plaintext fallback is
///   provider-AGNOSTIC (pre-migration installs only ever had one ambiguous
///   api_key value, since the shared-slot ambiguity this whole feature fixes
///   pre-dates per-provider disambiguation) — both providers may legitimately
///   read the same plaintext value until `migrate_v82` runs and splits it.
/// - Flag IS set → keychain only; miss = "not configured"
pub fn read_provider_key(conn: &Connection, runtime_type: &str) -> Option<String> {
    let snapshot = CredentialDbSnapshot::capture(conn);
    read_provider_key_from_snapshot(&snapshot, runtime_type)
}

/// Keychain-only counterpart to `read_provider_key` (D26, #2673 PR-A): takes a
/// pre-captured `CredentialDbSnapshot` instead of a live `&Connection`, so it can
/// run after `db.0` has been released (e.g. inside `tokio::task::spawn_blocking`).
/// Identical read-order/logging behavior to `read_provider_key` — see its doc
/// comment for the full Decision 5 contract.
pub fn read_provider_key_from_snapshot(
    snapshot: &CredentialDbSnapshot,
    runtime_type: &str,
) -> Option<String> {
    let Some(account) = provider_keychain_account(runtime_type) else {
        return None;
    };

    // C5 (#2584): explicit Result match — Err from the keychain must never be
    // collapsed into the same "miss" branch as a genuine Ok(None) absence. An
    // ACL-denied read or locked keychain is NOT "not configured" and logging it
    // as such makes real failures undiagnosable (that's exactly what happened in
    // #2584 — every case logged the identical generic "miss" message). `had_error`
    // tracks whether we already emitted the distinct READ ERROR log so the
    // fallthrough branches below never ALSO emit the generic "miss" message for
    // the same call — that would still be conflating the two cases in the logs
    // even though a distinct message was also printed.
    let mut had_error = false;
    let from_keychain = match crate::keychain::get_password(account) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_provider_key({}): keychain READ ERROR: {} — unavailable, not 'not configured'",
                runtime_type, e
            );
            had_error = true;
            None
        }
    };

    if from_keychain.is_some() {
        return from_keychain;
    }

    if snapshot.migration_complete {
        if !had_error {
            // Post-migration: genuine keychain miss (Ok(None)) = "not configured."
            // No plaintext fallback.
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_provider_key({}): keychain miss post-migration — not configured",
                runtime_type
            );
        }
        return None;
    }

    // Pre-migration fallback: read from plaintext ai_settings. Provider-agnostic
    // on purpose — see doc comment above.
    let plaintext = snapshot.plaintext_api_key.clone();
    if plaintext.is_some() {
        eprintln!(
            "[CREDENTIAL-MIGRATION] read_provider_key({}): using plaintext fallback (pre-migration)",
            runtime_type
        );
    }
    plaintext
}

/// Read the OC gateway bearer token.
///
/// Migration-aware read order (Decision 5):
/// - Flag NOT set → keychain first, fall back to plaintext ai_settings.api_key
/// - Flag IS set → keychain only; miss = "not configured"
pub fn read_gateway_token(conn: &Connection) -> Option<String> {
    let snapshot = CredentialDbSnapshot::capture(conn);
    read_gateway_token_from_snapshot(&snapshot)
}

/// Keychain-only counterpart to `read_gateway_token` (D26, #2673 PR-A) — see
/// `read_provider_key_from_snapshot` doc comment for the rationale.
pub fn read_gateway_token_from_snapshot(snapshot: &CredentialDbSnapshot) -> Option<String> {
    // C5 (#2584): explicit Result match — see read_provider_key doc comment.
    let mut had_error = false;
    let from_keychain = match crate::keychain::get_password(KEYCHAIN_ACCOUNT_GATEWAY_TOKEN) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_gateway_token: keychain READ ERROR: {} — unavailable, not 'not configured'",
                e
            );
            had_error = true;
            None
        }
    };

    if from_keychain.is_some() {
        return from_keychain;
    }

    if snapshot.migration_complete {
        if !had_error {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_gateway_token: keychain miss post-migration — not configured"
            );
        }
        return None;
    }

    let plaintext = snapshot.plaintext_api_key.clone();
    if plaintext.is_some() {
        eprintln!(
            "[CREDENTIAL-MIGRATION] read_gateway_token: using plaintext fallback (pre-migration)"
        );
    }
    plaintext
}

/// Read the Smart Features (internal) provider key.
///
/// Migration-aware read order (Decision 5):
/// - Flag NOT set → keychain first, fall back to plaintext ai_settings.internal_api_key
/// - Flag IS set → keychain only; miss = "not configured"
pub fn read_internal_key(conn: &Connection) -> Option<String> {
    // C5 (#2584): explicit Result match — see read_provider_key doc comment.
    let mut had_error = false;
    let from_keychain = match crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_internal_key: keychain READ ERROR: {} — unavailable, not 'not configured'",
                e
            );
            had_error = true;
            None
        }
    };

    if from_keychain.is_some() {
        return from_keychain;
    }

    if is_migration_complete(conn) {
        if !had_error {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_internal_key: keychain miss post-migration — not configured"
            );
        }
        return None;
    }

    let plaintext = crate::database::get_ai_setting(conn, "internal_api_key");
    if plaintext.is_some() {
        eprintln!(
            "[CREDENTIAL-MIGRATION] read_internal_key: using plaintext fallback (pre-migration)"
        );
    }
    plaintext
}

// ─── Public: Credential Write Paths (post-migration) ──────────────────────

/// Write a per-provider API key to its own keychain slot and clear the
/// legacy shared plaintext. Used by `ai_configure_and_save()` when
/// `runtime_type ∈ {openrouter, anthropic}` (B3, #2565).
///
/// `runtime_type` must be `"anthropic"` or `"openrouter"`; any other value
/// returns an `Err` — these are the only two key-based providers.
pub fn write_provider_key(conn: &Connection, runtime_type: &str, key: &str) -> Result<(), String> {
    let account = provider_keychain_account(runtime_type).ok_or_else(|| {
        format!(
            "write_provider_key: '{}' is not a key-based provider (expected anthropic or openrouter)",
            runtime_type
        )
    })?;

    crate::keychain::store_password(account, key)
        .map_err(|e| format!("Failed to store {} key in keychain: {}", runtime_type, e))?;
    // Clear any lingering legacy plaintext (pre-migration installs shared one
    // ambiguous api_key value across both providers).
    conn.execute(
        "DELETE FROM ai_settings WHERE key = 'api_key'",
        [],
    )
    .map_err(|e| format!("Failed to clear api_key from ai_settings: {}", e))?;
    eprintln!(
        "[CREDENTIAL-MIGRATION] write_provider_key({}): stored in keychain, plaintext cleared",
        runtime_type
    );
    Ok(())
}

/// Write the OC gateway token to keychain and clear it from ai_settings.
/// Used by ai_configure_and_save() when provider = openclaw.
pub fn write_gateway_token(conn: &Connection, token: &str) -> Result<(), String> {
    crate::keychain::store_password(KEYCHAIN_ACCOUNT_GATEWAY_TOKEN, token)
        .map_err(|e| format!("Failed to store gateway token in keychain: {}", e))?;
    // Clear any lingering plaintext
    conn.execute(
        "DELETE FROM ai_settings WHERE key = 'api_key'",
        [],
    )
    .map_err(|e| format!("Failed to clear api_key from ai_settings: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] write_gateway_token: stored in keychain, plaintext cleared");
    Ok(())
}

/// Write the internal key to keychain and clear it from ai_settings.
/// Used by set_ai_setting() when key = "internal_api_key" (post-migration).
pub fn write_internal_key(conn: &Connection, key: &str) -> Result<(), String> {
    crate::keychain::store_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY, key)
        .map_err(|e| format!("Failed to store internal key in keychain: {}", e))?;
    // Clear any lingering plaintext
    conn.execute(
        "DELETE FROM ai_settings WHERE key = 'internal_api_key'",
        [],
    )
    .map_err(|e| format!("Failed to clear internal_api_key from ai_settings: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] write_internal_key: stored in keychain, plaintext cleared");
    Ok(())
}

// ─── Brave Search API key (§4.6.8, Part 2, #2591, ADR-0064 D5) ──────────────
//
// `ai_brave_key` postdates ADR-0044 entirely, so unlike every read/write pair
// above these three functions have NO migration-flag branch and NO plaintext
// `ai_settings` fallback — keychain miss simply means "not configured"
// (§4.6.8: "There is no pre-migration plaintext fallback for this key... is
// keychain-only: miss means not configured").

/// Read the Brave Search API key. Keychain-only — no migration awareness, no
/// plaintext fallback (see section doc comment above). Mirrors the other
/// read_*_key functions' explicit `Result` match style (C5 #2584) but not
/// their migration-fallback *logic*.
pub fn read_brave_key(_conn: &Connection) -> Option<String> {
    match crate::keychain::get_password(KEYCHAIN_ACCOUNT_BRAVE_KEY) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_brave_key: keychain READ ERROR: {} — unavailable, not 'not configured'",
                e
            );
            None
        }
    }
}

/// Write the Brave Search API key to keychain. No plaintext `ai_settings`
/// counterpart ever existed for this key, so unlike `write_internal_key` /
/// `write_provider_key` / `write_gateway_token` there is no companion
/// plaintext-clear `DELETE` — that would be a no-op that misleadingly implies
/// a plaintext era this key never had.
pub fn write_brave_key(_conn: &Connection, key: &str) -> Result<(), String> {
    crate::keychain::store_password(KEYCHAIN_ACCOUNT_BRAVE_KEY, key)
        .map_err(|e| format!("Failed to store Brave key in keychain: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] write_brave_key: stored in keychain");
    Ok(())
}

/// Delete the Brave Search API key from keychain. Thin wrapper around
/// `keychain::delete_password` — named explicitly by §4.6.8 (A1 in the Part 2
/// scoping audit: this is the first `delete_*_key` wrapper at this module's
/// level; the other four accounts' disconnect paths call
/// `keychain::delete_password` directly from `commands.rs`. Simple, explicit
/// shape as specified; not extended to the other four in this PR.
pub fn delete_brave_key() -> Result<(), String> {
    crate::keychain::delete_password(KEYCHAIN_ACCOUNT_BRAVE_KEY)
        .map_err(|e| format!("Failed to delete Brave key from keychain: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] delete_brave_key: removed from keychain");
    Ok(())
}

/// ADR-0065 D4: Read Venice API key from keychain. Keychain-only — no
/// migration-flag branch, no plaintext fallback, matching `read_brave_key`'s
/// posture. Venice is a new provider with no legacy plaintext era.
pub fn read_venice_key(_conn: &Connection) -> Option<String> {
    match crate::keychain::get_password(KEYCHAIN_ACCOUNT_VENICE_KEY) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_venice_key: keychain READ ERROR: {} — unavailable, not 'not configured'",
                e
            );
            None
        }
    }
}

/// ADR-0065 D4: Keychain-only Venice key read from snapshot (D26, #2673 PA pattern).
/// Used by `build_runtime_connection_info` to mask the credential for the Settings card.
/// No migration-flag branch — Venice is post-migration-only (no legacy plaintext slot).
pub fn read_venice_key_from_snapshot(_snapshot: &CredentialDbSnapshot) -> Option<String> {
    match crate::keychain::get_password(KEYCHAIN_ACCOUNT_VENICE_KEY) {
        Ok(v) => v,
        Err(e) => {
            log_credential!(
                "[CREDENTIAL-MIGRATION] read_venice_key_from_snapshot: keychain READ ERROR: {} — unavailable",
                e
            );
            None
        }
    }
}

/// ADR-0065 D4: Write Venice API key to keychain. No plaintext counterpart
/// ever existed — no companion DELETE, matching `write_brave_key`'s posture.
pub fn write_venice_key(_conn: &Connection, key: &str) -> Result<(), String> {
    crate::keychain::store_password(KEYCHAIN_ACCOUNT_VENICE_KEY, key)
        .map_err(|e| format!("Failed to store Venice key in keychain: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] write_venice_key: stored in keychain");
    Ok(())
}

/// ADR-0065 D4: Delete Venice API key from keychain on disconnect.
pub fn delete_venice_key() -> Result<(), String> {
    crate::keychain::delete_password(KEYCHAIN_ACCOUNT_VENICE_KEY)
        .map_err(|e| format!("Failed to delete Venice key from keychain: {}", e))?;
    eprintln!("[CREDENTIAL-MIGRATION] delete_venice_key: removed from keychain");
    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use rusqlite::Connection;
    use serial_test::serial;
    use std::sync::Mutex;

    // Test serialization mutex — prevents keychain mock state leaking between tests
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn backup_accounts_include_preserved_legacy_provider_slot() {
        assert!(ALL_AI_KEYCHAIN_ACCOUNTS.contains(&KEYCHAIN_ACCOUNT_PROVIDER_KEY));
    }

    // ── Test 16 (§4.6.10, Part 2): Brave key keychain round-trip + backup inclusion ─

    #[test]
    fn test_ai_brave_key_is_in_all_ai_keychain_accounts() {
        assert!(ALL_AI_KEYCHAIN_ACCOUNTS.contains(&KEYCHAIN_ACCOUNT_BRAVE_KEY));
        assert_eq!(KEYCHAIN_ACCOUNT_BRAVE_KEY, "ai_brave_key");
    }

    #[test]
    #[serial(keychain_override)]
    fn test_write_brave_key_then_read_brave_key_round_trips_via_mock_keychain() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_in_memory_db();

        assert_eq!(read_brave_key(&conn), None, "must be None before any write");

        write_brave_key(&conn, "BSAtest1234567890").expect("write_brave_key should succeed against MockKeychain");
        assert_eq!(
            read_brave_key(&conn).as_deref(),
            Some("BSAtest1234567890"),
            "read_brave_key must return exactly what write_brave_key stored"
        );

        // write_brave_key must NOT touch ai_settings (no plaintext era for this key).
        assert_eq!(get_plain(&conn, "ai_brave_key"), None);
        assert_eq!(get_plain(&conn, "brave_key"), None);

    }

    #[test]
    #[serial(keychain_override)]
    fn test_delete_brave_key_clears_it() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_in_memory_db();

        write_brave_key(&conn, "BSAtoDelete999").unwrap();
        assert_eq!(read_brave_key(&conn).as_deref(), Some("BSAtoDelete999"));

        delete_brave_key().expect("delete_brave_key should succeed against MockKeychain");
        assert_eq!(read_brave_key(&conn), None, "read_brave_key must be None after delete_brave_key");

    }

    #[test]
    #[serial(keychain_override)]
    fn test_read_brave_key_has_no_migration_flag_branch() {
        // Unlike read_internal_key/read_provider_key/read_gateway_token,
        // read_brave_key must NOT fall back to plaintext ai_settings regardless
        // of the migration flag's state — this key postdates ADR-0044 entirely.
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_in_memory_db();

        // Migration flag NOT set (pre-migration state) + a plaintext value sitting
        // under a hypothetical legacy key name — read_brave_key must still return
        // None, because there is no plaintext read path for it at all.
        set_plain(&conn, "brave_key", "should-never-be-read");
        assert_eq!(read_brave_key(&conn), None);

    }

    fn setup_in_memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    fn set_plain(conn: &Connection, key: &str, val: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, val],
        )
        .unwrap();
    }

    fn get_plain(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM ai_settings WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .ok()
    }

    // ─── Test 1: State A — fresh migrate (Decision 2) ─────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_a_fresh_migrate_all_three_credentials() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // Seed plaintext credentials (pre-migration state)
        set_plain(&conn, "provider", "anthropic");
        set_plain(&conn, "api_key", "sk-ant-test123");
        set_plain(&conn, "internal_api_key", "sk-or-internal456");

        // Run migration
        run_credential_migration(&conn);

        // Assert: all three land in keychain — B3 (#2565): anthropic routes to its
        // OWN per-provider account now, not the legacy shared slot.
        let provider_key = crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY)
            .unwrap();
        assert_eq!(
            provider_key.as_deref(),
            Some("sk-ant-test123"),
            "anthropic key should be in its own keychain account"
        );
        assert!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY)
                .unwrap()
                .is_none(),
            "openrouter account must stay empty — migration is provider-specific, not shared"
        );

        let internal_key = crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY)
            .unwrap();
        assert_eq!(
            internal_key.as_deref(),
            Some("sk-or-internal456"),
            "internal key should be in keychain"
        );

        // Assert: plaintext cleared from ai_settings
        assert!(
            get_plain(&conn, "api_key").is_none(),
            "api_key should be cleared from ai_settings"
        );
        assert!(
            get_plain(&conn, "internal_api_key").is_none(),
            "internal_api_key should be cleared from ai_settings"
        );

        // Assert: migration flag set
        assert!(
            is_migration_complete(&conn),
            "migration flag should be set after State A"
        );

    }

    // ─── Test 1b: State A — gateway token variant ─────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_a_gateway_token_migrates_to_correct_account() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        set_plain(&conn, "provider", "openclaw");
        set_plain(&conn, "api_key", "oc-token-abc");

        run_credential_migration(&conn);

        // Gateway token should be in gateway account
        let gateway_token = crate::keychain::get_password(KEYCHAIN_ACCOUNT_GATEWAY_TOKEN)
            .unwrap();
        assert_eq!(
            gateway_token.as_deref(),
            Some("oc-token-abc"),
            "gateway token should be in gateway keychain account"
        );

        // Neither per-provider account should be populated for an openclaw provider.
        assert!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY)
                .unwrap()
                .is_none(),
            "anthropic account should be empty for openclaw provider"
        );
        assert!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY)
                .unwrap()
                .is_none(),
            "openrouter account should be empty for openclaw provider"
        );

        // Plaintext cleared
        assert!(
            get_plain(&conn, "api_key").is_none(),
            "api_key plaintext should be cleared"
        );

    }

    // ─── Test 2: State C — partial / interrupted ──────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_c_partial_interrupted_plaintext_wins_on_diverge() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // Keychain was written in a prior partial attempt (stale value) — in the
        // OWN per-provider account, since B3 (#2565) means the routing logic never
        // writes to the legacy shared slot anymore.
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "stale-keychain-value")
            .unwrap();
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY, "stale-internal-value")
            .unwrap();

        // Plaintext is still present (crash before clear), with DIFFERENT values
        set_plain(&conn, "provider", "openrouter");
        set_plain(&conn, "api_key", "sk-or-correct-value");
        set_plain(&conn, "internal_api_key", "sk-ant-correct-internal");

        // Flag NOT set (partial migration state)
        // Run migration
        run_credential_migration(&conn);

        // Assert: plaintext wins — keychain overwritten with plaintext value
        let provider_key = crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY)
            .unwrap();
        assert_eq!(
            provider_key.as_deref(),
            Some("sk-or-correct-value"),
            "plaintext wins on diverge — keychain should have plaintext value"
        );

        let internal_key = crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY)
            .unwrap();
        assert_eq!(
            internal_key.as_deref(),
            Some("sk-ant-correct-internal"),
            "plaintext wins on diverge — internal key should have plaintext value"
        );

        // Assert: plaintext cleared after reconciliation
        assert!(
            get_plain(&conn, "api_key").is_none(),
            "api_key should be cleared after State C reconciliation"
        );
        assert!(
            get_plain(&conn, "internal_api_key").is_none(),
            "internal_api_key should be cleared after State C reconciliation"
        );

        // Assert: flag set
        assert!(
            is_migration_complete(&conn),
            "migration flag should be set after State C reconciliation"
        );

    }

    // ─── Test 3: Post-migration keychain miss = "not configured" ──────────

    #[test]
    #[serial(keychain_override)]
    fn test_post_migration_keychain_miss_returns_not_configured() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // Set migration flag (post-migration regime)
        set_plain(&conn, MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE);

        // Put a plaintext value in ai_settings to prove we don't fall back to it
        set_plain(&conn, "api_key", "should-never-be-returned");
        set_plain(&conn, "internal_api_key", "internal-should-never-be-returned");

        // Keychain is empty (miss)

        // Assert: read functions return None, not the plaintext value — checked for
        // BOTH per-provider slots (B3, #2565).
        let anthropic_key = read_provider_key(&conn, "anthropic");
        assert!(
            anthropic_key.is_none(),
            "post-migration keychain miss for anthropic key must return None (not plaintext)"
        );
        let openrouter_key = read_provider_key(&conn, "openrouter");
        assert!(
            openrouter_key.is_none(),
            "post-migration keychain miss for openrouter key must return None (not plaintext)"
        );

        let gateway_token = read_gateway_token(&conn);
        assert!(
            gateway_token.is_none(),
            "post-migration keychain miss for gateway token must return None (not plaintext)"
        );

        let internal_key = read_internal_key(&conn);
        assert!(
            internal_key.is_none(),
            "post-migration keychain miss for internal key must return None (not plaintext)"
        );

    }

    // ─── Test 4: State E — fresh install ──────────────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_e_fresh_install_sets_flag_no_op() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // No plaintext, no keychain, no flag — fresh install
        run_credential_migration(&conn);

        // Flag should be set (nothing to migrate is a completed migration)
        assert!(
            is_migration_complete(&conn),
            "migration flag should be set on fresh install (State E)"
        );

        // No unexpected keychain entries
        assert!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap().is_none());
        assert!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap().is_none());

    }

    // ─── Test 5: Failed keychain write — preserve plaintext, no flag ──────

    #[test]
    #[serial(keychain_override)]
    fn test_failed_keychain_write_preserves_plaintext_no_flag() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Use a failing keychain mock
        let _kc = KeychainGuard::set_with(Box::new(FailingKeychain));

        let conn = setup_in_memory_db();

        set_plain(&conn, "provider", "anthropic");
        set_plain(&conn, "api_key", "sk-ant-preserve-me");
        set_plain(&conn, "internal_api_key", "sk-or-preserve-internal");

        run_credential_migration(&conn);

        // Assert: plaintext preserved (keychain write failed)
        let preserved = get_plain(&conn, "api_key");
        assert_eq!(
            preserved.as_deref(),
            Some("sk-ant-preserve-me"),
            "plaintext must be preserved when keychain write fails"
        );

        let internal_preserved = get_plain(&conn, "internal_api_key");
        assert_eq!(
            internal_preserved.as_deref(),
            Some("sk-or-preserve-internal"),
            "internal plaintext must be preserved when keychain write fails"
        );

        // Assert: migration flag NOT set
        assert!(
            !is_migration_complete(&conn),
            "migration flag must NOT be set when keychain write fails"
        );
    }

    // ─── Test 6: State B — idempotent no-op when already migrated ─────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_b_already_migrated_is_noop() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // State B: flag set, keychain populated, no plaintext
        set_plain(&conn, MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE);
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY, "existing-provider-key")
            .unwrap();

        // Run migration — should be a complete no-op
        run_credential_migration(&conn);

        // Keychain value should be unchanged
        let provider_key = crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap();
        assert_eq!(
            provider_key.as_deref(),
            Some("existing-provider-key"),
            "State B: keychain value should be unchanged"
        );

    }

    // ─── Test 7: Migration-aware read falls back pre-migration ────────────

    #[test]
    #[serial(keychain_override)]
    fn test_pre_migration_read_falls_back_to_plaintext() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // Pre-migration: flag NOT set, keychain empty, plaintext present
        set_plain(&conn, "api_key", "sk-ant-plaintext-fallback");
        set_plain(&conn, "internal_api_key", "sk-or-internal-fallback");

        // Both per-provider reads legitimately fall back to the SAME ambiguous
        // plaintext value pre-migration — there's only ever one plaintext api_key,
        // and disambiguation only happens once migrate_v82 runs (B3 §2c).
        let anthropic_key = read_provider_key(&conn, "anthropic");
        assert_eq!(
            anthropic_key.as_deref(),
            Some("sk-ant-plaintext-fallback"),
            "pre-migration anthropic read should fall back to plaintext"
        );
        let openrouter_key = read_provider_key(&conn, "openrouter");
        assert_eq!(
            openrouter_key.as_deref(),
            Some("sk-ant-plaintext-fallback"),
            "pre-migration openrouter read should fall back to the same plaintext (ambiguous pre-migration)"
        );

        let internal_key = read_internal_key(&conn);
        assert_eq!(
            internal_key.as_deref(),
            Some("sk-or-internal-fallback"),
            "pre-migration internal key read should fall back to plaintext"
        );

    }

    // ─── Failing keychain mock (for Test 5) ───────────────────────────────

    struct FailingKeychain;

    impl crate::keychain::KeychainProvider for FailingKeychain {
        fn store_password(&self, _account: &str, _password: &str) -> Result<(), String> {
            Err("Simulated keychain write failure".to_string())
        }

        fn get_password(&self, _account: &str) -> Result<Option<String>, String> {
            // Return None (no entry) — simulates empty/unavailable keychain
            Ok(None)
        }

        fn delete_password(&self, _account: &str) -> Result<(), String> {
            Err("Simulated keychain delete failure".to_string())
        }
    }

    // ─── C5 (#2584): Err-vs-Ok(None) distinction on all three read functions ──
    //
    // Before the fix, `.ok().flatten()` collapsed a real keychain `Err` (ACL
    // denial, locked keychain, etc.) into the exact same code path — and log
    // message — as a genuine `Ok(None)` absence. These tests capture stderr and
    // assert the two cases produce visibly different diagnostics, and that an
    // `Err` never gets logged with the generic "miss ... not configured" message.

    /// Joins all logs captured on this thread since the last drain into one
    /// string, for substring assertions.
    fn captured_log_text() -> String {
        super::test_take_captured_logs().join("\n")
    }

    #[test]
    #[serial(keychain_override)]
    fn test_read_provider_key_err_logs_distinct_from_ok_none() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let conn = setup_in_memory_db();
        set_plain(&conn, MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE);

        // Case 1: genuine Ok(None) miss.
        let mut _kc = KeychainGuard::set();
        let _ = super::test_take_captured_logs(); // drain any prior residue
        let result = read_provider_key(&conn, "anthropic");
        assert!(result.is_none(), "genuine miss must return None");
        let miss_log = captured_log_text();
        assert!(
            miss_log.contains("keychain miss post-migration"),
            "Ok(None) miss must log the 'miss' message, got: {}",
            miss_log
        );
        assert!(
            !miss_log.contains("READ ERROR"),
            "Ok(None) miss must NOT log a READ ERROR message, got: {}",
            miss_log
        );

        // Case 2: real Err from the keychain (ACL denial / locked keychain).
        let erroring = crate::keychain::MockKeychain::new();
        erroring.fail_get_for(
            KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
            crate::keychain::KeychainFailureMode::Error("simulated ACL denial".to_string()),
        );
        drop(_kc);
        _kc = KeychainGuard::set_with(Box::new(erroring));
        let _ = super::test_take_captured_logs();
        let result = read_provider_key(&conn, "anthropic");
        assert!(
            result.is_none(),
            "Err path currently surfaces as None to callers, but must log distinctly"
        );
        let err_log = captured_log_text();
        assert!(
            err_log.contains("READ ERROR") && err_log.contains("simulated ACL denial"),
            "Err from keychain must log a distinct READ ERROR message with the underlying error, got: {}",
            err_log
        );
        assert!(
            !err_log.contains("keychain miss post-migration"),
            "Err from keychain must NOT be logged with the generic 'miss ... not configured' message — that's exactly the #2584 defect, got: {}",
            err_log
        );
    }

    #[test]
    #[serial(keychain_override)]
    fn test_read_gateway_token_err_logs_distinct_from_ok_none() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let conn = setup_in_memory_db();
        set_plain(&conn, MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE);

        let mut _kc = KeychainGuard::set();
        let _ = super::test_take_captured_logs();
        assert!(read_gateway_token(&conn).is_none());
        let miss_log = captured_log_text();
        assert!(miss_log.contains("keychain miss post-migration"), "got: {}", miss_log);
        assert!(!miss_log.contains("READ ERROR"), "got: {}", miss_log);

        let erroring = crate::keychain::MockKeychain::new();
        erroring.fail_get_for(
            KEYCHAIN_ACCOUNT_GATEWAY_TOKEN,
            crate::keychain::KeychainFailureMode::Error("simulated locked keychain".to_string()),
        );
        drop(_kc);
        _kc = KeychainGuard::set_with(Box::new(erroring));
        let _ = super::test_take_captured_logs();
        assert!(read_gateway_token(&conn).is_none());
        let err_log = captured_log_text();
        assert!(
            err_log.contains("READ ERROR") && err_log.contains("simulated locked keychain"),
            "got: {}",
            err_log
        );
        assert!(!err_log.contains("keychain miss post-migration"), "got: {}", err_log);
    }

    /// ADR-0065 D4: write_venice_key → read_venice_key round-trip via mock keychain.
    /// Venice is a post-migration-only provider — no plaintext fallback, no migration flag.
    #[test]
    #[serial(keychain_override)]
    fn test_write_venice_key_then_read_venice_key_round_trips_via_mock_keychain() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let conn = setup_in_memory_db();
        let _kc = KeychainGuard::set();

        assert_eq!(read_venice_key(&conn), None, "must be None before any write");
        // write_venice_key must NOT touch ai_settings (no plaintext era for this key).
        assert_eq!(get_plain(&conn, "ai_venice_key"), None);

        write_venice_key(&conn, "VEN-test-key-9876543210").expect("write_venice_key must succeed against MockKeychain");
        assert_eq!(
            read_venice_key(&conn).as_deref(),
            Some("VEN-test-key-9876543210"),
            "read_venice_key must return exactly what write_venice_key stored"
        );

        // write must NOT have written to ai_settings
        assert_eq!(get_plain(&conn, "ai_venice_key"), None, "write_venice_key must never touch ai_settings");
    }

    /// ADR-0065 D4: delete_venice_key clears the key from keychain.
    #[test]
    #[serial(keychain_override)]
    fn test_delete_venice_key_clears_it() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let conn = setup_in_memory_db();
        let _kc = KeychainGuard::set();

        write_venice_key(&conn, "VEN-to-delete-999").unwrap();
        assert_eq!(read_venice_key(&conn).as_deref(), Some("VEN-to-delete-999"));
        delete_venice_key().expect("delete_venice_key must succeed against MockKeychain");
        assert_eq!(read_venice_key(&conn), None, "read_venice_key must be None after delete_venice_key");
    }

    /// ADR-0065 D4: Venice key is in ALL_AI_KEYCHAIN_ACCOUNTS for backup.
    #[test]
    fn test_ai_venice_key_is_in_all_ai_keychain_accounts() {
        assert_eq!(KEYCHAIN_ACCOUNT_VENICE_KEY, "ai_venice_key");
        assert!(
            ALL_AI_KEYCHAIN_ACCOUNTS.contains(&"ai_venice_key"),
            "ai_venice_key must be in ALL_AI_KEYCHAIN_ACCOUNTS for backup export"
        );
    }

    /// ADR-0065 D4: `read_venice_key_from_snapshot` must NOT fall back to plaintext
    /// `ai_settings.api_key` even when the migration flag is unset (pre-migration
    /// regime) and a stale plaintext value is present. Venice postdates ADR-0044
    /// entirely, so there is no plaintext era for it to fall back to — unlike
    /// `read_provider_key_from_snapshot`, which legitimately does fall back for
    /// anthropic/openrouter in this same regime.
    #[test]
    #[serial(keychain_override)]
    fn test_read_venice_key_from_snapshot_no_plaintext_fallback_when_migration_flag_unset() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = setup_in_memory_db();

        // Migration flag UNSET (pre-migration regime) — deliberately NOT calling
        // set_plain(MIGRATION_FLAG_KEY, ...).
        // Stale plaintext ai_settings.api_key present, as a pre-migration install
        // would have.
        set_plain(&conn, "api_key", "should-never-be-returned-for-venice");

        let snapshot = CredentialDbSnapshot::capture(&conn);
        assert!(
            !snapshot.migration_complete,
            "precondition: migration flag must be unset for this test"
        );

        let result = read_venice_key_from_snapshot(&snapshot);
        assert_eq!(
            result, None,
            "read_venice_key_from_snapshot must return None even pre-migration with a stale \
             plaintext api_key present — Venice has no plaintext fallback in any migration regime"
        );

    }

    /// ADR-0065 D4: provider_keychain_account returns Venice's keychain slot.
    #[test]
    fn test_provider_keychain_account_venice_returns_correct_slot() {
        assert_eq!(
            provider_keychain_account("venice"),
            Some(KEYCHAIN_ACCOUNT_VENICE_KEY),
            "venice must map to KEYCHAIN_ACCOUNT_VENICE_KEY"
        );
        // Existing providers still work
        assert_eq!(provider_keychain_account("anthropic"), Some(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY));
        assert_eq!(provider_keychain_account("openrouter"), Some(KEYCHAIN_ACCOUNT_OPENROUTER_KEY));
        assert_eq!(provider_keychain_account("ollama"), None);
        assert_eq!(provider_keychain_account("openclaw"), None);
    }

    #[test]
    #[serial(keychain_override)]
    fn test_read_internal_key_err_logs_distinct_from_ok_none() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let conn = setup_in_memory_db();
        set_plain(&conn, MIGRATION_FLAG_KEY, MIGRATION_FLAG_COMPLETE);

        let mut _kc = KeychainGuard::set();
        let _ = super::test_take_captured_logs();
        assert!(read_internal_key(&conn).is_none());
        let miss_log = captured_log_text();
        assert!(miss_log.contains("keychain miss post-migration"), "got: {}", miss_log);
        assert!(!miss_log.contains("READ ERROR"), "got: {}", miss_log);

        let erroring = crate::keychain::MockKeychain::new();
        erroring.fail_get_for(
            KEYCHAIN_ACCOUNT_INTERNAL_KEY,
            crate::keychain::KeychainFailureMode::Error("simulated interaction-not-allowed".to_string()),
        );
        drop(_kc);
        _kc = KeychainGuard::set_with(Box::new(erroring));
        let _ = super::test_take_captured_logs();
        assert!(read_internal_key(&conn).is_none());
        let err_log = captured_log_text();
        assert!(
            err_log.contains("READ ERROR") && err_log.contains("simulated interaction-not-allowed"),
            "got: {}",
            err_log
        );
        assert!(!err_log.contains("keychain miss post-migration"), "got: {}", err_log);
    }
}

// ─── C50 (#3171 PR 1): Feature-Level Key Consolidation tests ────────────────
#[cfg(test)]
mod feature_key_consolidation_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use rusqlite::Connection;
    use serial_test::serial;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn
    }

    fn set_plain(conn: &Connection, key: &str, val: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, val],
        )
        .unwrap();
    }

    fn get_plain(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM ai_settings WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .ok()
    }

    fn flag_is_set(conn: &Connection) -> bool {
        get_plain(conn, FEATURE_KEY_CONSOLIDATION_FLAG_KEY).as_deref()
            == Some(FEATURE_KEY_CONSOLIDATION_COMPLETE)
    }

    // ── Semantic Search: embeddings_api_key (plaintext) ──────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_embeddings_api_key_backfills_openrouter_connection_when_absent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-semantic-search-key");

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-semantic-search-key"),
            "the Semantic Search key must be backfilled into the openrouter connection slot"
        );
        assert_eq!(
            get_plain(&conn, "embeddings_api_key"),
            None,
            "the plaintext embeddings_api_key row must be GONE after migration, not merely unread"
        );
        assert!(flag_is_set(&conn), "flag must be set after a successful pass");
    }

    #[test]
    #[serial(keychain_override)]
    fn test_embeddings_api_key_discarded_when_connection_already_present() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        // Provider Connections already has an openrouter key — the canonical source.
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "sk-or-v1-CONNECTION-KEY")
            .unwrap();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-stale-semantic-search-key");

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-CONNECTION-KEY"),
            "the connection's own key must win — never overwritten by the discarded feature copy"
        );
        assert_eq!(
            get_plain(&conn, "embeddings_api_key"),
            None,
            "the discarded plaintext embeddings_api_key row must be GONE, not merely unused"
        );
    }

    // #3171 PR 2 fast-follow (Reed's #3172 audit §4): the discard path's
    // new emptiness/whitespace guard. An empty or whitespace-only
    // connection keychain value must be treated as "no connection key
    // yet" — the feature's real key backfills INTO it (overwriting the
    // blank), rather than being discarded in favor of a connection slot
    // that holds nothing usable.
    #[test]
    #[serial(keychain_override)]
    fn test_embeddings_api_key_backfills_when_connection_key_is_empty_string() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        // A connection slot that "has" a value, but it's blank — never a
        // usable credential.
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "").unwrap();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-semantic-search-key");

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-semantic-search-key"),
            "an empty connection key must not win — the feature's real key must backfill into it"
        );
        assert_eq!(
            get_plain(&conn, "embeddings_api_key"),
            None,
            "the feature-level copy must still be cleared once backfilled"
        );
    }

    #[test]
    #[serial(keychain_override)]
    fn test_embeddings_api_key_backfills_when_connection_key_is_whitespace_only() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "   ").unwrap();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-semantic-search-key");

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-semantic-search-key"),
            "a whitespace-only connection key must not win — the feature's real key must backfill into it"
        );
    }
    // Mutation guard for the two tests immediately above, named
    // explicitly: drop the `.is_some_and(|v| !v.trim().is_empty())`
    // guard (revert to bare `connection_key.is_some()`) — reddens both
    // (the blank connection key would "win" and the real feature key
    // would be discarded instead of backfilled). Actually run below
    // this test module's own mutation-test commit note.

    #[test]
    #[serial(keychain_override)]
    fn test_embeddings_api_key_absent_is_a_noop() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        run_feature_key_consolidation(&conn);
        assert_eq!(read_provider_key(&conn, "openrouter"), None);
        assert!(flag_is_set(&conn));
    }

    // ── Smart Features: ai_internal_key (keychain, shared OR/Anthropic slot) ───

    #[test]
    #[serial(keychain_override)]
    fn test_smart_features_internal_key_backfills_openrouter_connection_when_absent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "internal_provider", "openrouter");
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY, "sk-or-v1-smart-features-key")
            .unwrap();

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-smart-features-key"),
            "Smart Features' openrouter key must be backfilled into the connection slot"
        );
        assert_eq!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY).unwrap(),
            None,
            "the ai_internal_key keychain account must be GONE after migration, not merely unread"
        );
        assert_eq!(get_plain(&conn, "internal_api_key"), None);
    }

    #[test]
    #[serial(keychain_override)]
    fn test_smart_features_internal_key_backfills_anthropic_connection_when_absent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "internal_provider", "anthropic");
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY, "sk-ant-smart-features-key")
            .unwrap();

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "anthropic").as_deref(),
            Some("sk-ant-smart-features-key")
        );
        assert_eq!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY).unwrap(),
            None
        );
    }

    #[test]
    #[serial(keychain_override)]
    fn test_smart_features_internal_key_discarded_when_connection_already_present() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "internal_provider", "openrouter");
        crate::keychain::store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "sk-or-v1-CONNECTION-KEY")
            .unwrap();
        crate::keychain::store_password(
            KEYCHAIN_ACCOUNT_INTERNAL_KEY,
            "sk-or-v1-stale-smart-features-key",
        )
        .unwrap();

        run_feature_key_consolidation(&conn);

        assert_eq!(
            read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-v1-CONNECTION-KEY")
        );
        assert_eq!(
            crate::keychain::get_password(KEYCHAIN_ACCOUNT_INTERNAL_KEY).unwrap(),
            None,
            "the discarded ai_internal_key must be GONE, not merely unused"
        );
    }

    #[test]
    #[serial(keychain_override)]
    fn test_smart_features_internal_key_untouched_when_provider_is_ollama() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "internal_provider", "ollama");
        // No ai_internal_key is ever written for ollama (it has no key) — nothing to migrate.
        run_feature_key_consolidation(&conn);
        assert!(flag_is_set(&conn));
    }

    // ── Idempotency ────────────────────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_second_pass_after_flag_set_is_a_true_noop() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let conn = setup_db();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-first-pass-key");
        run_feature_key_consolidation(&conn);
        assert!(flag_is_set(&conn));

        // A new plaintext row appearing after the flag is already set (e.g. a
        // reverted frontend momentarily re-wrote it) must NOT be touched by the
        // fast-path — that's what "idempotent, flagged" means for the one-shot
        // shape `run_credential_migration` itself already uses.
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-should-not-be-touched");
        run_feature_key_consolidation(&conn);
        assert_eq!(
            get_plain(&conn, "embeddings_api_key").as_deref(),
            Some("sk-or-v1-should-not-be-touched"),
            "a second pass after the flag is set must be a true no-op, touching nothing"
        );
    }

    // ── Matt's named mutation: a failed write/confirm must preserve the feature ──
    // ── copy and must NOT set the flag (never clear before a confirmed write) ──

    #[test]
    #[serial(keychain_override)]
    fn test_backfill_write_failure_preserves_the_feature_level_copy_and_does_not_set_flag() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let conn = setup_db();
        set_plain(&conn, "embeddings_api_key", "sk-or-v1-should-survive-a-failed-write");

        let failing = crate::keychain::MockKeychain::new();
        failing.fail_store_for(
            KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
            crate::keychain::KeychainFailureMode::Error("simulated keychain write failure".to_string()),
        );
        let _kc = KeychainGuard::set_with(Box::new(failing));

        run_feature_key_consolidation(&conn);

        assert_eq!(
            get_plain(&conn, "embeddings_api_key").as_deref(),
            Some("sk-or-v1-should-survive-a-failed-write"),
            "a failed keychain write must preserve the feature-level plaintext copy — \
             never clear before a confirmed write-back"
        );
        assert!(
            !flag_is_set(&conn),
            "the flag must NOT be set when a sub-migration fails — must retry next launch"
        );
    }
}
