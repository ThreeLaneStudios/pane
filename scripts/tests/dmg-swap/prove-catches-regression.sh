#!/usr/bin/env bash
# =============================================================================
# prove-catches-regression.sh — proof that test-swap-path.sh is not vacuous
# =============================================================================
# Same proof shape as scripts/tests/check-spec-first/run-tests.sh (#3015):
# a check is only trustworthy once you've shown it actually fails on the
# exact regression it's meant to catch, not just that it passes on the
# happy path.
#
# Runs test-swap-path.sh four times:
#   1. Normally — must exit 0 (all three fixes are in place).
#   2. With DMG_SWAP_TEST_SKIP_UDRW_CONVERT=1 — must exit non-zero
#      (reproduces #3049's original root cause: attaching a UDZO image
#      -readwrite directly).
#   3. With DMG_SWAP_TEST_BREAK_STAGING_NAME=1 — must exit non-zero
#      (reproduces the #3049 fast-follow's bug 1: a non-.dmg-suffixed
#      hdiutil convert -o path, which silently lands the file at the
#      wrong name instead of the one every later check looks for).
#   4. With DMG_SWAP_TEST_BREAK_TRAP=1 — must exit non-zero EVEN THOUGH
#      the run is otherwise fully successful (reproduces the #3049
#      fast-follow's bug 2: a bare `&&`-chain EXIT trap that overrides a
#      real `exit 0` with its own last statement's status). This run's
#      own body is expected to print every "✅" success line just like
#      Run 1 — the only difference is the final exit code, which is
#      exactly the bug this run exists to prove.
#
# Not part of the PR-gating CI job (that just runs test-swap-path.sh once,
# normally, on every relevant PR) — this script is the one-time proof that
# the gate is real, run manually and pasted into the PR description.
#
# Usage: scripts/tests/dmg-swap/prove-catches-regression.sh
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST="$SCRIPT_DIR/test-swap-path.sh"

echo "########################################"
echo "# Run 1: normal (all fixes in place) — expect PASS (exit 0)"
echo "########################################"
NORMAL_OUT=$(bash "$TEST" 2>&1)
NORMAL_EXIT=$?
echo "$NORMAL_OUT"
echo ""
echo "Run 1 exit code: $NORMAL_EXIT"
echo ""

echo "########################################"
echo "# Run 2: DMG_SWAP_TEST_SKIP_UDRW_CONVERT=1 (original #3049 bug reintroduced) — expect FAIL (non-zero)"
echo "########################################"
SKIP_OUT=$(DMG_SWAP_TEST_SKIP_UDRW_CONVERT=1 bash "$TEST" 2>&1)
SKIP_EXIT=$?
echo "$SKIP_OUT"
echo ""
echo "Run 2 exit code: $SKIP_EXIT"
echo ""

echo "########################################"
echo "# Run 3: DMG_SWAP_TEST_BREAK_STAGING_NAME=1 (fast-follow bug 1 reintroduced) — expect FAIL (non-zero)"
echo "########################################"
NAME_OUT=$(DMG_SWAP_TEST_BREAK_STAGING_NAME=1 bash "$TEST" 2>&1)
NAME_EXIT=$?
echo "$NAME_OUT"
echo ""
echo "Run 3 exit code: $NAME_EXIT"
echo ""

echo "########################################"
echo "# Run 4: DMG_SWAP_TEST_BREAK_TRAP=1 (fast-follow bug 2 reintroduced) — expect FAIL (non-zero) despite a clean run"
echo "########################################"
TRAP_OUT=$(DMG_SWAP_TEST_BREAK_TRAP=1 bash "$TEST" 2>&1)
TRAP_EXIT=$?
echo "$TRAP_OUT"
echo ""
echo "Run 4 exit code: $TRAP_EXIT"
echo ""

echo "########################################"
echo "# Verdict"
echo "########################################"
PASS=0
if [ "$NORMAL_EXIT" -eq 0 ]; then
  echo "✅ Run 1 (fix in place) passed as expected"
else
  echo "❌ Run 1 (fix in place) FAILED unexpectedly — the test itself is broken"
  PASS=1
fi

if [ "$SKIP_EXIT" -ne 0 ]; then
  echo "✅ Run 2 (original #3049 bug reintroduced) failed as expected — the test catches it"
else
  echo "❌ Run 2 PASSED — the test is vacuous, it does not catch #3049's original bug"
  PASS=1
fi

if [ "$NAME_EXIT" -ne 0 ]; then
  echo "✅ Run 3 (fast-follow bug 1 reintroduced) failed as expected — dmg_convert_or_die catches it"
else
  echo "❌ Run 3 PASSED — dmg_convert_or_die did not catch a non-.dmg output path"
  PASS=1
fi

if echo "$TRAP_OUT" | grep -q "✅ DMG swap-path test passed" && [ "$TRAP_EXIT" -ne 0 ]; then
  echo "✅ Run 4 (fast-follow bug 2 reintroduced) printed a full PASS body but still exited non-zero — this is the exact false-failure bug, proving the broken trap shape is what's catching it (not a real failure)"
elif [ "$TRAP_EXIT" -ne 0 ]; then
  echo "⚠️  Run 4 exited non-zero but didn't print the expected PASS body — re-check output above"
  PASS=1
else
  echo "❌ Run 4 PASSED with exit 0 — the broken trap shape did not reproduce the false-failure bug here"
  PASS=1
fi

exit $PASS
