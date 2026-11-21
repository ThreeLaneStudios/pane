#!/usr/bin/env bash
# =============================================================================
# CI Guard Unit Tests — G1–G6
# =============================================================================
# Tests: 2 per guard (catches violation, passes clean) = 12 total (G1–G5)
#        + 2 G6 tests = 14 total
# Each test runs rg against a fixture file and asserts expected output.
#
# Run: scripts/ci-guards/tests/run-tests.sh
# Also called by: run-guards.sh test
# Also called by: .github/workflows/forbid-hardcoded-constants.yml (self-check step)
# =============================================================================
set -euo pipefail

# Locate ripgrep — check PATH first, then known locations (local dev, CI)
if ! command -v rg &>/dev/null; then
  for candidate in \
    /opt/homebrew/bin/rg \
    /usr/bin/rg \
    /usr/local/bin/rg \
    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/arm64-darwin/rg" \
    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-darwin/rg"; do
    if [[ -x "$candidate" ]]; then
      export PATH="$(dirname "$candidate"):${PATH}"
      break
    fi
  done
fi

if ! command -v rg &>/dev/null; then
  echo "❌ ripgrep (rg) not found. Install via: brew install ripgrep" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FIXTURES="${SCRIPT_DIR}/fixtures"

PASS=0
FAIL=0

# ---------------------------------------------------------------------------
# Utility
# ---------------------------------------------------------------------------
assert_match() {
  local test_name="$1" pattern="$2" file="$3"
  if rg -q -e "$pattern" "$file" 2>/dev/null; then
    echo "  ✅ PASS: ${test_name}"
    ((PASS++)) || true
  else
    echo "  ❌ FAIL: ${test_name}"
    echo "     Expected pattern to match in ${file}:"
    echo "     Pattern: ${pattern}"
    ((FAIL++)) || true
  fi
}

assert_no_match() {
  local test_name="$1" pattern="$2" file="$3"
  if rg -q -e "$pattern" "$file" 2>/dev/null; then
    echo "  ❌ FAIL: ${test_name}"
    echo "     Expected NO match but pattern matched in ${file}:"
    echo "     Pattern: ${pattern}"
    rg -e "$pattern" "$file" | head -5
    ((FAIL++)) || true
  else
    echo "  ✅ PASS: ${test_name}"
    ((PASS++)) || true
  fi
}

assert_match_filtered() {
  # Matches pattern but then applies a grep -v filter; expects at least one line remaining
  local test_name="$1" pattern="$2" file="$3" filter="$4"
  local raw_matches
  raw_matches=$(rg -e "$pattern" "$file" 2>/dev/null || true)
  local filtered
  filtered=$(echo "$raw_matches" | grep -v "$filter" | grep -v '^$' || true)
  if [[ -n "$filtered" ]]; then
    echo "  ✅ PASS: ${test_name}"
    ((PASS++)) || true
  else
    echo "  ❌ FAIL: ${test_name}"
    echo "     Expected filtered results to be non-empty in ${file}"
    echo "     Pattern: ${pattern}, Filter: ${filter}"
    ((FAIL++)) || true
  fi
}

assert_no_match_filtered() {
  # Matches pattern and applies grep -v filter; expects zero lines remaining
  local test_name="$1" pattern="$2" file="$3" filter="$4"
  local raw_matches
  raw_matches=$(rg -e "$pattern" "$file" 2>/dev/null || true)
  local filtered
  filtered=$(echo "$raw_matches" | grep -v "$filter" | grep -v '^$' || true)
  if [[ -z "$filtered" ]]; then
    echo "  ✅ PASS: ${test_name}"
    ((PASS++)) || true
  else
    echo "  ❌ FAIL: ${test_name}"
    echo "     Expected NO filtered results in ${file} but got:"
    echo "$filtered"
    ((FAIL++)) || true
  fi
}

# ---------------------------------------------------------------------------
# G1 Tests: hardcoded temperature literals
# ---------------------------------------------------------------------------
echo ""
echo "── G1: Hardcoded temperature literals ──────────────────────────────"

# Test 1: G1 catches violation in Rust fixture
assert_match \
  "G1-rust-violation: catches 'temperature: 0.7' in Rust" \
  '"temperature"\s*:\s*[0-9]' \
  "${FIXTURES}/g1_violation.rs"

# Test 2: G1 does not trigger on clean Rust fixture
assert_no_match \
  "G1-rust-clean: does not fire when temperature is a variable" \
  '"temperature"\s*:\s*[0-9]' \
  "${FIXTURES}/g1_clean.rs"

# Test 3: G1 catches violation in TypeScript fixture
assert_match \
  "G1-ts-violation: catches '\"temperature\": 0' in TypeScript" \
  '"temperature"\s*:\s*[0-9]' \
  "${FIXTURES}/g1_violation.ts"

# Test 4: G1 does not trigger on clean TypeScript fixture
assert_no_match \
  "G1-ts-clean: does not fire when temperature is config.temperature" \
  '"temperature"\s*:\s*[0-9]' \
  "${FIXTURES}/g1_clean.ts"

# ---------------------------------------------------------------------------
# G2 Tests: hardcoded max_tokens in streaming paths
# ---------------------------------------------------------------------------
echo ""
echo "── G2: Hardcoded max_tokens literals ───────────────────────────────"

# Test 5: G2 catches violation fixture
assert_match \
  "G2-violation: catches '\"max_tokens\": 4096' literal" \
  '"max_tokens"\s*:\s*[0-9]+' \
  "${FIXTURES}/g2_violation.rs"

# Test 6: G2 does not trigger on clean fixture (variable, not literal)
assert_no_match \
  "G2-clean: does not fire when max_tokens is request.max_tokens" \
  '"max_tokens"\s*:\s*[0-9]+' \
  "${FIXTURES}/g2_clean.rs"

# ---------------------------------------------------------------------------
# G3 Tests: model ID literals in production Rust
# ---------------------------------------------------------------------------
echo ""
echo "── G3: Model ID literals in production Rust ────────────────────────"

# Test 7: G3 catches violation fixture
assert_match \
  "G3-violation: catches '\"anthropic/claude-opus-4-6\"' literal" \
  '"anthropic/claude-[a-z0-9._-]+"' \
  "${FIXTURES}/g3_violation.rs"

# Test 8: G3 does not trigger on clean fixture
assert_no_match \
  "G3-clean: does not fire when no model ID literal present" \
  '"anthropic/claude-[a-z0-9._-]+"' \
  "${FIXTURES}/g3_clean.rs"

# ---------------------------------------------------------------------------
# G4 Tests: frontend model IDs without PICKER-SEED annotation
# ---------------------------------------------------------------------------
echo ""
echo "── G4: Frontend model IDs without PICKER-SEED annotation ───────────"

# Test 9: G4 catches violation after filtering — violation file has model IDs, none annotated
assert_match_filtered \
  "G4-violation: catches unannotated model IDs in TypeScript" \
  "anthropic/claude-[a-z0-9._-]+" \
  "${FIXTURES}/g4_violation.ts" \
  "// PICKER-SEED:"

# Test 10: G4 passes clean — all model IDs have PICKER-SEED annotation
assert_no_match_filtered \
  "G4-clean: all model IDs annotated with PICKER-SEED are not flagged" \
  "anthropic/claude-[a-z0-9._-]+" \
  "${FIXTURES}/g4_clean.ts" \
  "// PICKER-SEED:"

# ---------------------------------------------------------------------------
# G5 Tests: context-window literals outside context_budget.rs
# ---------------------------------------------------------------------------
echo ""
echo "── G5: Context-window integer literals ─────────────────────────────"

# Test 11: G5 catches violation (file not named context_budget.rs)
assert_match \
  "G5-violation: catches 200_000 and 128_000 in non-context_budget file" \
  '\b200_000\b|\b128_000\b' \
  "${FIXTURES}/g5_violation.rs"

# Test 12: G5 clean fixture has no window literals at all
assert_no_match \
  "G5-clean: no window literals in clean fixture" \
  '\b200_000\b|\b128_000\b|\b200000\b|\b128000\b' \
  "${FIXTURES}/g5_clean.rs"

# ---------------------------------------------------------------------------
# G6 Tests: OC protocol version literals in gateway source
# ---------------------------------------------------------------------------
echo ""
echo "── G6: OC protocol integer literals in src-gateway/src/ ──────────────────"

# Test 13: G6 catches violation fixture (hardcoded protocol literals including in test module)
assert_match \
  "G6-violation: catches hardcoded \"protocol\": 4 literal (including in test module)" \
  '"protocol"\s*:\s*[0-9]' \
  "${FIXTURES}/g6_violation.rs"

# Test 14: G6 does not trigger on clean fixture (protocol expressed as named constant)
assert_no_match \
  "G6-clean: does not fire when protocol is a named constant (OC_PROTOCOL_MIN)" \
  '"protocol"\s*:\s*[0-9]' \
  "${FIXTURES}/g6_clean.rs"

# ---------------------------------------------------------------------------
# G8 Tests: unguarded Keychain-delete / rm-rf-~/Pane patterns
# ---------------------------------------------------------------------------
echo ""
echo "── G8: Unguarded Keychain-delete / rm-rf-~/Pane patterns ────────────"

# Test 15: G8 catches the unguarded delete-generic-password candidate
assert_match \
  "G8-violation: catches 'security delete-generic-password' candidate line" \
  'security delete-generic-password' \
  "${FIXTURES}/g8_violation.sh"

# Test 16: G8 catches the unguarded rm -rf ~/Pane candidate
assert_match \
  "G8-violation: catches 'rm -rf ~/Pane' candidate line" \
  'rm -rf.*~/Pane' \
  "${FIXTURES}/g8_violation.sh"

# Test 17: G8's guard pattern IS present in the clean fixture, above both
# candidate lines (full guard/skip logic is exercised by run-guards.sh g8
# directly against these fixtures in CI; here we just pin that the guard
# pattern itself is detectable so a future edit can't silently drop it).
assert_match \
  "G8-clean: PANE_TEST_ROOT guard pattern present ahead of the candidates" \
  '\$\{PANE_TEST_ROOT:\?' \
  "${FIXTURES}/g8_clean.sh"

# Test 18: G8's path-shape anchor does not fire on Pane.app / scratch-dir /
# PaneGateway-dmg paths — none of these are the dangerous ~/Pane shape.
assert_no_match \
  "G8-clean: rm -rf of Pane.app/scratch-dir/PaneGateway-dmg never matches the anchored pattern" \
  'rm -rf.*~/Pane($|["'"'"'[:space:]/])' \
  "${FIXTURES}/g8_path_shape_safe.sh"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "────────────────────────────────────────────────────────────────────"
TOTAL=$((PASS + FAIL))
echo "CI Guard Tests: ${PASS}/${TOTAL} passed"
if [[ "$FAIL" -gt 0 ]]; then
  echo "❌ ${FAIL} test(s) FAILED — guards are not working correctly"
  exit 1
else
  echo "✅ All ${PASS} tests passed"
  exit 0
fi
