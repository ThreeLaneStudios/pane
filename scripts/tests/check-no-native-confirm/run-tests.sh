#!/usr/bin/env bash
# =============================================================================
# check-no-native-confirm.sh unit tests
# =============================================================================
# Each case runs the production script against a synthetic fixture
# directory (never the real src/ tree) and asserts the exit code. The
# required case (per #3044 item 6's own instruction) is "the check catches
# a planted violation" — not a vacuous always-pass.
#
# Run: scripts/tests/check-no-native-confirm/run-tests.sh
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SCRIPT="$REPO_ROOT/scripts/check-no-native-confirm.sh"
FIXTURES="$SCRIPT_DIR/fixtures"

PASS=0
FAIL=0

run_case() {
  local name="$1" expected_exit="$2" fixture_subdir="$3"
  local output exit_code
  output=$(REPO_ROOT="$REPO_ROOT" "$SCRIPT" "$FIXTURES/$fixture_subdir" 2>&1)
  exit_code=$?

  if [ "$exit_code" -eq "$expected_exit" ]; then
    echo "  ✅ PASS: ${name} (exit=${exit_code})"
    ((PASS++)) || true
  else
    echo "  ❌ FAIL: ${name} (expected exit=${expected_exit}, got exit=${exit_code})"
    echo "     --- script output ---"
    echo "${output//$'\n'/$'\n'     }"
    echo "     ----------------------"
    ((FAIL++)) || true
  fi
}

echo "Running check-no-native-confirm.sh test suite..."
echo ""

# 1. REQUIRED: a planted bare confirm( call -> FAILS the check. This is the
# test that would have caught the three real sites (TaskEditModal.svelte,
# TaskFullPage.svelte, TaskList.svelte) this check exists to catch — proving
# the check actually discriminates, not passing vacuously on an empty/clean
# fixture tree alone.
run_case \
  "case1: planted bare confirm( call -> FAIL" \
  1 \
  "planted-bare"

# 2. REQUIRED: a planted window.confirm( call -> FAILS the check too (the
# other real-world call shape, same native dialog).
run_case \
  "case2: planted window.confirm( call -> FAIL" \
  1 \
  "planted-window"

# 3. Clean tree (ConfirmDialog usage only, no native confirm at all) -> PASS.
run_case \
  "case3: clean tree, ConfirmDialog only -> PASS" \
  0 \
  "clean"

# 4. confirm( mentioned only in a // comment -> PASS (not a real call).
run_case \
  "case4: confirm( only in a // comment -> PASS" \
  0 \
  "comment-only-line"

# 5. confirm( mentioned only in a /* */ block comment -> PASS.
run_case \
  "case5: confirm( only in a /* */ block comment -> PASS" \
  0 \
  "comment-only-block"

# 6. confirm( mentioned only in a <!-- --> template comment -> PASS. This is
# the exact shape (NewProviderModal.svelte, ProviderConnectionCard.svelte,
# Onboarding.svelte) that a naive quote-tracking stripper got wrong while
# building this check — real regression coverage, not a hypothetical.
run_case \
  "case6: confirm( only in a <!-- --> template comment -> PASS" \
  0 \
  "comment-only-html"

# 7. A file with an apostrophe in plain HTML template TEXT (not a JS
# string), followed later by a real <!-- --> comment mentioning confirm( —
# must still resolve to PASS. Proves the apostrophe-in-text bug (found and
# fixed while building this check) stays fixed.
run_case \
  "case7: apostrophe in template text + later HTML comment mentioning confirm( -> PASS" \
  0 \
  "apostrophe-in-text"

# 8. A file that declares its OWN local `confirm` function and calls it —
# must NOT be flagged (shadowed, not the native global). Mirrors
# AgentPickerPopover.svelte's real shape in this repo.
run_case \
  "case8: locally-shadowed confirm() (own function, zero args) -> PASS" \
  0 \
  "shadowed"

# 9. alert( is explicitly NOT forbidden — a file using only alert() passes.
run_case \
  "case9: alert() only (not forbidden) -> PASS" \
  0 \
  "alert-only"

# 10. Empty/non-existent directory -> PASS (nothing to scan).
run_case \
  "case10: nonexistent directory -> PASS (nothing to scan)" \
  0 \
  "does-not-exist"

echo ""
echo "============================================================"
echo "Results: ${PASS} passed, ${FAIL} failed"
echo "============================================================"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
exit 0
