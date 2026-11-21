#!/usr/bin/env bash
# Usage: smoke-launch.sh fresh|upgrade
# Exits 0 on successful launch + clean shutdown
# Exit codes: 0 (success), 2 (bad args), 3 (app exited before ready), 4 (timeout)
#
# === Tauri Data Path Isolation ===
# Pane uses a custom get_pane_root() function in src-tauri/src/database.rs
# that checks PANE_TEST_ROOT env var BEFORE falling back to ~/Pane.
# This allows us to isolate the app's data directory without `HOME` override,
# which would bypass macOS-specific paths that Tauri may use.
#
# Implementation: Set PANE_TEST_ROOT to a temp workspace.
# The app will use $PANE_TEST_ROOT/Pane as its root directory.
#
# === Keychain Isolation (separate mechanism — do not conflate) ===
# Filesystem isolation (above) and Keychain isolation are NOT the same thing.
# A prior incident wiped the real macOS Keychain because this script's
# cleanup loop (and keychain.rs::SERVICE_NAME) used the literal production
# service name "com.threelane.pane" unconditionally — PANE_TEST_ROOT has no
# effect on the Keychain at all unless the service name itself branches on
# it. keychain.rs::service_name() now returns "com.threelane.pane.test"
# whenever PANE_TEST_ROOT is set (which this script always sets, above);
# KEYCHAIN_SERVICE below is hardcoded to match that test literal exactly —
# defense in depth, so even if a future edit (or a human running this script
# by hand) somehow unsets PANE_TEST_ROOT first, the hardcoded test-service
# literal alone still prevents collateral damage to the real Keychain.

set -euo pipefail

# ─── Panic Context Printer ──────────────────────────────────────────────────
# A Rust panic that crosses an extern "C" FFI boundary (e.g. tao's AppKit
# delegate callbacks) aborts via SIGABRT with a full backtrace. The backtrace
# can run 30+ frames, long enough that a plain `tail -50` of stderr may push
# the actual `thread 'main' panicked at <file>:<line>: <message>` line off
# the top of the captured window before anyone reads it (seen 6/6 times on
# real #3222 failures). This prints that line, plus the 3 lines after it, in
# ADDITION to the existing tail — never instead of it.
#
# NOTE: under `set -euo pipefail`, a `grep` with no match exits 1, and
# pipefail propagates the *rightmost non-zero* exit code through the
# pipeline even when a later stage (e.g. `head`) exits 0. An unguarded
# `grep ... | head -1` here would abort this diagnostic — and the smoke run
# it is diagnosing — the first time a failure log had no panic line. The
# assignment below is guarded with `|| true` for exactly that reason.
print_panic_context() {
    local log_file="$1"
    local panic_line
    panic_line=$(grep -n "panicked at" "$log_file" 2>/dev/null | head -1) || true

    if [ -z "$panic_line" ]; then
        echo "[SMOKE] (no 'panicked at' line found in $log_file)"
        return 0
    fi

    local line_num="${panic_line%%:*}"
    echo "[SMOKE] === Panic context (line ${line_num}, plus 3 lines after) ==="
    sed -n "${line_num},$((line_num + 3))p" "$log_file"
}

# ─── Schema Policy ──────────────────────────────────────────────────────────
# Controls what the upgrade-mode smoke test asserts.
#
# pre-launch  — No migration arm exists for the current schema bump (§1.10).
#               The upgrade fixture triggers SCHEMA_MISMATCH, which is the
#               CORRECT and EXPECTED outcome.  Finding it = PASS.
#
# post-launch — A migration arm has been shipped.  The app should apply it
#               and emit a ready signal ("migration applied" etc.).
#               Finding a ready signal = PASS.  SCHEMA_MISMATCH = FAIL.
#
# One-liner flip when launching: export SCHEMA_POLICY=post-launch, or edit
# the default below.
SCHEMA_POLICY="${SCHEMA_POLICY:-post-launch}"

MODE="${1:-}"
if [ -z "$MODE" ]; then
    echo "Usage: smoke-launch.sh fresh|upgrade"
    exit 2
fi

WORKSPACE=$(mktemp -d)
trap "rm -rf '$WORKSPACE'" EXIT
LOG="/tmp/smoke-stderr-${MODE}.log"

# Export PANE_TEST_ROOT so the app uses our isolated workspace
export PANE_TEST_ROOT="$WORKSPACE"

case "$MODE" in
  fresh)
    # Empty workspace, app initializes fresh DB
    echo "[SMOKE] Mode: fresh-DB launch (empty workspace)"
    ;;
  upgrade)
    # Pre-populate workspace with upgrade fixture
    CURRENT_VERSION=$(grep -oE 'const SCHEMA_VERSION: &str = "[0-9]+"' src-tauri/src/migrations.rs | grep -oE '[0-9]+')
    PREV_VERSION=$((CURRENT_VERSION - 1))
    echo "[SMOKE] Mode: upgrade-DB launch (v${PREV_VERSION}→v${CURRENT_VERSION} migration)"
    
    # Copy upgrade fixture to workspace
    FIXTURE="tests/fixtures/upgrade-from-v${PREV_VERSION}.db"
    if [ ! -f "$FIXTURE" ]; then
      echo "[SMOKE] ❌ Fixture not found: $FIXTURE"
      exit 2
    fi
    
    # Create .pane directory in workspace
    # PANE_TEST_ROOT is used directly by get_pane_root(), so DB is at $WORKSPACE/.pane/pane.db
    mkdir -p "$WORKSPACE/.pane"
    
    # Copy fixture to workspace DB location
    cp "$FIXTURE" "$WORKSPACE/.pane/pane.db"
    echo "[SMOKE] ✓ Copied v${PREV_VERSION} fixture to workspace"
    ;;
  *)
    echo "ERROR: Unknown mode: $MODE"
    echo "Usage: smoke-launch.sh fresh|upgrade"
    exit 2
    ;;
esac

echo "[SMOKE] Workspace: $WORKSPACE"
echo "[SMOKE] Log: $LOG"

# Refuse to run the Keychain cleanup below without isolation in effect.
# PANE_TEST_ROOT is already exported unconditionally above, so this is
# belt-and-suspenders defense-in-depth (CI guard G8 requires a textual
# PANE_TEST_ROOT check above any delete-generic-password/rm-rf-Pane line in
# scripts/ or tests/) — if a future edit ever reorders/removes that export,
# this still refuses loudly instead of silently proceeding unisolated.
: "${PANE_TEST_ROOT:?PANE_TEST_ROOT must be set before touching the Keychain}"

# Clean up keychain items from the TEST-ISOLATED com.threelane.pane.test
# service only (never the real "com.threelane.pane" service — see the
# Keychain Isolation note above). The CI debug binary has a different code
# signature than the release app, so keychain items written by the release
# app trigger blocking access dialogs. Delete all known accounts to prevent
# hangs. (smoke test doesn't need real credentials)
KEYCHAIN_SERVICE="com.threelane.pane.test"
KEYCHAIN_ACCOUNTS=(
    "ai_provider_key"
    "ai_gateway_token"
    "ai_internal_key"
    "sync_master_key"
    "supabase_access_token"
    "supabase_refresh_token"
)
for ACCT in "${KEYCHAIN_ACCOUNTS[@]}"; do
    security delete-generic-password -s "$KEYCHAIN_SERVICE" -a "$ACCT" 2>/dev/null || true
done
echo "[SMOKE] ✓ Cleaned keychain items for $KEYCHAIN_SERVICE"

echo "[SMOKE] Launching app..."

# Launch app with isolated test root, capture stderr
./src-tauri/target/debug/pane 2> "$LOG" &
APP_PID=$!

echo "[SMOKE] App PID: $APP_PID"

# ─── Wait loop ──────────────────────────────────────────────────────────────
# Upgrade mode in pre-launch: assert SCHEMA_MISMATCH (no migration arm, §1.10)
# Upgrade mode in post-launch OR fresh mode: assert ready signal
TIMEOUT=30
ELAPSED=0
READY=0

if [ "$MODE" = "upgrade" ] && [ "$SCHEMA_POLICY" = "pre-launch" ]; then
  # ── Pre-launch upgrade assertion ──────────────────────────────────────────
  # The fixture is an old-version DB; no migration arm exists.
  # SCHEMA_MISMATCH in stderr = the app correctly rejected the stale schema.
  echo "[SMOKE] Policy: pre-launch — expecting SCHEMA_MISMATCH"

  while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
    if grep -q "SCHEMA_MISMATCH" "$LOG" 2>/dev/null; then
      echo "[SMOKE] ✓ SCHEMA_MISMATCH detected — correct pre-launch outcome (§1.10)"
      kill -TERM "$APP_PID" 2>/dev/null || true
      wait "$APP_PID" 2>/dev/null || true
      echo "[SMOKE] ✓ Smoke launch complete"
      echo "[SMOKE] Log captured at: $LOG"
      exit 0
    fi

    # If a ready signal appears instead, an unauthorized migration arm is present
    if grep -qE "(Schema verified|Schema created successfully|migration applied)" "$LOG" 2>/dev/null; then
      echo "[SMOKE] ❌ Unexpected ready signal in pre-launch mode"
      echo "[SMOKE]    A migration arm appears to exist — confirm §1.10 compliance"
      echo "[SMOKE]    or flip SCHEMA_POLICY=post-launch when shipping the migration."
      echo "[SMOKE] === Last 50 lines of stderr ==="
      tail -50 "$LOG"
      print_panic_context "$LOG"
      kill "$APP_PID" 2>/dev/null || true
      wait "$APP_PID" 2>/dev/null || true
      exit 5
    fi

    if ! kill -0 "$APP_PID" 2>/dev/null; then
      echo "[SMOKE] ❌ App exited before SCHEMA_MISMATCH was emitted"
      echo "[SMOKE] === Last 50 lines of stderr ==="
      tail -50 "$LOG"
      print_panic_context "$LOG"
      exit 3
    fi

    sleep 1
    ELAPSED=$((ELAPSED + 1))
    if [ $((ELAPSED % 5)) -eq 0 ]; then
      echo "[SMOKE] Waiting for SCHEMA_MISMATCH... ${ELAPSED}s elapsed"
    fi
  done

  echo "[SMOKE] ❌ SCHEMA_MISMATCH not seen within ${TIMEOUT}s"
  echo "[SMOKE] === Last 50 lines of stderr ==="
  tail -50 "$LOG"
  print_panic_context "$LOG"
  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  exit 4

else
  # ── Standard ready-signal assertion (fresh mode, or post-launch upgrade) ──
  if [ "$MODE" = "upgrade" ]; then
    echo "[SMOKE] Policy: post-launch — expecting ready signal (migration applied)"
  fi

  while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
    # Matches: "Schema verified", "Schema created successfully", or "vN → vM migration applied"
    if grep -qE "(Schema verified|Schema created successfully|migration applied)" "$LOG" 2>/dev/null; then
      READY=1
      echo "[SMOKE] ✓ Ready signal detected"
      break
    fi

    if ! kill -0 "$APP_PID" 2>/dev/null; then
      echo "[SMOKE] ❌ App exited before ready signal"
      echo "[SMOKE] === Last 50 lines of stderr ==="
      tail -50 "$LOG"
      print_panic_context "$LOG"
      exit 3
    fi

    sleep 1
    ELAPSED=$((ELAPSED + 1))
    if [ $((ELAPSED % 5)) -eq 0 ]; then
      echo "[SMOKE] Waiting for ready signal... ${ELAPSED}s elapsed"
    fi
  done

  if [ "$READY" -eq 0 ]; then
    echo "[SMOKE] ❌ App did not reach ready state within ${TIMEOUT}s"
    echo "[SMOKE] === Last 50 lines of stderr ==="
    tail -50 "$LOG"
    print_panic_context "$LOG"
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    exit 4
  fi

  # Clean shutdown
  echo "[SMOKE] Sending SIGTERM to app..."
  kill -TERM "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
fi

echo "[SMOKE] ✓ Smoke launch complete"
echo "[SMOKE] Log captured at: $LOG"
echo "[SMOKE] Workspace: $WORKSPACE"

exit 0
