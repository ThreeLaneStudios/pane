#!/usr/bin/env bash
# =============================================================================
# CI Guard Runner — Pane hardcoded-constant guards (G1–G6)
# =============================================================================
# Usage:
#   run-guards.sh g1            — run guard G1 only
#   run-guards.sh g2            — run guard G2 only
#   run-guards.sh all           — run all guards (G1–G6)
#   run-guards.sh test          — run unit tests against fixtures
#
# Mode (per-guard, configured via env var):
#   G1_MODE=report   (default) — log violations, never fail the build
#   G1_MODE=fail               — log violations AND exit non-zero
#   Same for G2_MODE … G6_MODE
#
# Output format per violation:
#   file:line — matched literal — GUARD_ID — mode:MODE — audit:AUDIT_REF
#
# Allowlist (per-guard, via inline comment on same line as violation):
#   G1 / G2: // CI-ALLOW: hardcoded-sampling
#   G3:       // CI-ALLOW: hardcoded-model
#   G4:       // PICKER-SEED: <description>   (annotation, not allowlist)
#   G5:       // CI-ALLOW: hardcoded-window
#   G6:       (no allowlist; constants are the only sanctioned pattern)
#
# Reed must approve any allowlist addition in audit.
# Do NOT add any allowlists in the initial landing PR (#1320).
#
# Spec: docs/dev-hygiene-playbook.md — Appendix E: CI Guard Catalog (G1–G6)
# Audit: ~/Shared/ExampleApp/docs/audits/2026-05-31-hardcoded-models-audit.md §7
# Audit: ~/Shared/ExampleApp/docs/audits/2026-05-31-1336-protocol-durability-audit.md §8
# =============================================================================
set -euo pipefail

# Locate ripgrep — check PATH first, then known local-dev locations
if ! command -v rg &>/dev/null; then
  for _rg_candidate in \
    /opt/homebrew/bin/rg \
    /usr/bin/rg \
    /usr/local/bin/rg \
    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/arm64-darwin/rg" \
    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-darwin/rg"; do
    if [[ -x "$_rg_candidate" ]]; then
      export PATH="$(dirname "$_rg_candidate"):${PATH}"
      break
    fi
  done
fi

if ! command -v rg &>/dev/null; then
  echo "❌ ripgrep (rg) not found in PATH or known locations. Install: brew install ripgrep" >&2
  exit 1
fi

REPO_ROOT="${REPO_ROOT:-$(git -C "$(dirname "$0")" rev-parse --show-toplevel)}"
AUDIT_REF="docs/dev-hygiene-playbook.md#appendix-e-ci-guard-catalog-g1g5"
AUDIT_DOC="~/Shared/ExampleApp/docs/audits/2026-05-31-hardcoded-models-audit.md §7"

# Track overall exit code — stays 0 unless a guard in fail mode has violations
OVERALL_EXIT=0

# ---------------------------------------------------------------------------
# Utility: print_header / print_pass / print_fail / print_report_only
# ---------------------------------------------------------------------------
print_header() {
  local guard_id="$1" desc="$2" mode="$3"
  echo ""
  echo "┌─────────────────────────────────────────────────────────────────"
  echo "│ ${guard_id}: ${desc}"
  echo "│ Mode: ${mode} | Spec: ${AUDIT_REF}"
  echo "├─────────────────────────────────────────────────────────────────"
}

print_footer_pass() {
  local guard_id="$1"
  echo "│ ✅ No violations found"
  echo "└─ ${guard_id} PASS"
}

print_footer_report() {
  local guard_id="$1" count="$2"
  echo "│"
  echo "│ 📋 REPORT-ONLY: ${count} violation(s) — build NOT failed (mode=report)"
  echo "│    Fix tracked in downstream issues per #536 Phase 1d sequence"
  echo "└─ ${guard_id} REPORT (${count} violation(s))"
}

print_footer_fail() {
  local guard_id="$1" count="$2"
  echo "│"
  echo "│ ❌ FAIL: ${count} violation(s) — build failed (mode=fail)"
  echo "└─ ${guard_id} FAIL (${count} violation(s))"
}

# ---------------------------------------------------------------------------
# run_guard <GUARD_ID> <DESCRIPTION> <ALLOWLIST_TAG> <rg output>
# Processes rg output: filters allowlisted lines, formats violations,
# then applies mode (report = always exit 0, fail = exit 1 if violations).
# ---------------------------------------------------------------------------
run_guard() {
  local guard_id="$1"
  local description="$2"
  local allowlist_tag="$3"
  local rg_output="$4"

  local mode_var="G${guard_id#G}_MODE"
  local mode="${!mode_var:-report}"

  print_header "$guard_id" "$description" "$mode"

  if [[ -z "$rg_output" ]]; then
    print_footer_pass "$guard_id"
    return 0
  fi

  local count=0
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue

    # Skip lines containing the allowlist tag (inline CI-ALLOW / PICKER-SEED)
    if [[ -n "$allowlist_tag" ]] && [[ "$line" == *"$allowlist_tag"* ]]; then
      echo "│ ⏭ ALLOWLISTED: $line"
      continue
    fi

    # rg output with --no-heading -n: filepath:linenum:content
    local filepath lineno content
    filepath=$(echo "$line" | cut -d: -f1)
    lineno=$(echo "$line" | cut -d: -f2)
    content=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//')

    echo "│ VIOLATION: ${filepath}:${lineno} — ${content} — ${guard_id} — mode:${mode} — audit:${AUDIT_REF}"
    ((count++)) || true
  done <<< "$rg_output"

  if [[ "$count" -eq 0 ]]; then
    print_footer_pass "$guard_id"
    return 0
  fi

  if [[ "$mode" == "fail" ]]; then
    print_footer_fail "$guard_id" "$count"
    OVERALL_EXIT=1
    return 1
  else
    print_footer_report "$guard_id" "$count"
    return 0
  fi
}

# ---------------------------------------------------------------------------
# Guard implementations
# Each function runs rg with the exact patterns from audit §7 / issue #1320,
# pipes to run_guard for formatting and mode handling.
# ---------------------------------------------------------------------------

guard_g1() {
  local description="Forbid hardcoded temperature literals in non-test Rust/TS source"

  # Rust: both JSON body key and struct field assignment
  local rust_output
  rust_output=$(rg --no-heading -n --type rust \
    -e '"temperature"\s*:\s*[0-9]' \
    -e 'temperature:\s*[0-9]' \
    --glob '!*_tests.rs' \
    --glob '!*tests*.rs' \
    --glob '!*test*.rs' \
    --glob '!target/**' \
    "${REPO_ROOT}/src-tauri/src/" 2>/dev/null || true)

  # TypeScript/Svelte: JSON body temperature literals
  local ts_output
  ts_output=$(rg --no-heading -n \
    --type-add 'frontend:*.{ts,tsx,svelte}' -t frontend \
    -e '"temperature"\s*:\s*[0-9]' \
    -e 'temperature:\s*[0-9]' \
    --glob '!node_modules/**' \
    --glob '!build/**' \
    --glob '!.svelte-kit/**' \
    --glob '!dist/**' \
    --glob '!__tests__/**' \
    --glob '!*.test.ts' \
    "${REPO_ROOT}/src/" 2>/dev/null || true)

  local combined_output
  combined_output=$(printf '%s\n%s' "$rust_output" "$ts_output" | grep -v '^$' || true)

  run_guard "G1" "$description" "CI-ALLOW: hardcoded-sampling" "$combined_output"
}

guard_g2() {
  local description="Forbid hardcoded max_tokens integer literals in streaming-path JSON bodies"

  # Pattern: "max_tokens": <integer_literal> in Rust AI source (streaming paths: stream_openrouter, stream_anthropic)
  # rg can't scope to function bodies; all AI source is scanned. Expected violations are in stream_ functions.
  # Internal sub-calls (reranker=100, extraction=1024, etc.) also flagged — fix via named constants in #1321.
  local rg_output
  rg_output=$(rg --no-heading -n --type rust \
    '"max_tokens"\s*:\s*[0-9]+' \
    --glob '!*_tests.rs' \
    --glob '!*tests*.rs' \
    --glob '!*test*.rs' \
    --glob '!target/**' \
    "${REPO_ROOT}/src-tauri/src/ai/" 2>/dev/null || true)

  run_guard "G2" "$description" "CI-ALLOW: hardcoded-sampling" "$rg_output"
}

guard_g3() {
  local description="Forbid model ID literals in production Rust outside context_budget.rs"

  local rg_output
  rg_output=$(rg --no-heading -n --type rust \
    -e '"anthropic/claude-[a-z0-9._-]+"' \
    -e '"claude-[a-z0-9._-]+"' \
    -e '"openai/gpt-[a-z0-9._+.-]+"' \
    -e '"google/gemini-[a-z0-9._-]+"' \
    --glob '!*_tests.rs' \
    --glob '!*tests*.rs' \
    --glob '!*test*.rs' \
    --glob '!*context_budget*' \
    --glob '!target/**' \
    "${REPO_ROOT}/src-tauri/src/" 2>/dev/null || true)

  # Also exclude gateway test files
  rg_output=$(echo "$rg_output" | grep -v '/tests/' | grep -v '_test\.' || true)

  run_guard "G3" "$description" "CI-ALLOW: hardcoded-model" "$rg_output"
}

guard_g4() {
  local description="Forbid model IDs in frontend without // PICKER-SEED: annotation (warn)"

  local rg_output
  rg_output=$(rg --no-heading -n \
    --type-add 'frontend:*.{ts,tsx,svelte}' -t frontend \
    -e "anthropic/claude-[a-z0-9._-]+" \
    -e "openai/gpt-[0-9a-z._-]+" \
    -e "google/gemini-[0-9a-z._-]+" \
    --glob '!node_modules/**' \
    --glob '!build/**' \
    --glob '!.svelte-kit/**' \
    --glob '!dist/**' \
    "${REPO_ROOT}/src/" 2>/dev/null || true)

  # Lines with // PICKER-SEED: are intentionally annotated — filter them out
  rg_output=$(echo "$rg_output" | grep -v '// PICKER-SEED:' || true)

  run_guard "G4" "$description" "" "$rg_output"
}

guard_g5() {
  local description="Forbid context-window integer literals outside context_budget.rs (warn)"

  local rg_output
  rg_output=$(rg --no-heading -n --type rust \
    -e '\b200_000\b' -e '\b200000\b' \
    -e '\b128_000\b' -e '\b128000\b' \
    -e '\b1_000_000\b' -e '\b1000000\b' \
    -e '\b100_000\b' -e '\b100000\b' \
    --glob '!*_tests.rs' \
    --glob '!*tests*.rs' \
    --glob '!*test*.rs' \
    --glob '!target/**' \
    "${REPO_ROOT}/src-tauri/src/ai/" 2>/dev/null | grep -v 'context_budget' || true)

  run_guard "G5" "$description" "CI-ALLOW: hardcoded-window" "$rg_output"
}

guard_g7() {
  local description="Forbid window.addEventListener for Tauri-emitted pane events in .svelte files"

  # pane-note-changed is emitted exclusively from Rust via Tauri IPC (notes.rs, ai/commands.rs).
  # window.addEventListener never fires for Tauri events — this is a silent class bug (#2215).
  # Components MUST use Tauri's listen() for Tauri-emitted events (see Sidebar.svelte pattern).
  #
  # Tauri-emitted event names caught by this guard (DO NOT add DOM CustomEvent names here):
  #   pane-note-changed  → notes.rs line ~1959, ai/commands.rs
  # If additional Rust-emitted events are added, extend the -e pattern list and this comment.
  #
  # Allowlist: // CI-ALLOW: tauri-listener-exception (requires Reed approval + written rationale)
  local rg_output
  rg_output=$(rg --no-heading -n \
    --type-add 'svelte:*.svelte' -t svelte \
    -e "window\.addEventListener\('pane-note-changed'" \
    --glob '!node_modules/**' \
    --glob '!build/**' \
    --glob '!.svelte-kit/**' \
    --glob '!dist/**' \
    "${REPO_ROOT}/src/" 2>/dev/null || true)

  run_guard "G7" "$description" "CI-ALLOW: tauri-listener-exception" "$rg_output"
}

guard_g8() {
  local description="Forbid unguarded security-delete-generic-password / rm -rf of a real ~/Pane path in scripts/ or tests/ (#3057 follow-up — same incident class as the smoke-launch.sh Keychain wipe)"

  # Candidate dangerous lines. Anchored to the literal path shape per the
  # audit's own caution: a bare `rm -rf.*Pane` substring match would also
  # catch `Pane.app` (a DMG-mount build artifact, not user data) — the
  # trailing character class below requires the path to end exactly at
  # "Pane" (end of string/quote/slash/whitespace), not continue into
  # ".app"/"Gateway"/etc.
  local candidates
  candidates=$(rg --no-heading -n \
    -e 'security delete-generic-password' \
    -e 'rm -rf.*~/Pane($|["'"'"'[:space:]/])' \
    -e 'rm -rf.*\$HOME/Pane($|["'"'"'[:space:]/])' \
    --glob '!node_modules/**' \
    --glob '!target/**' \
    --glob '!**/ci-guards/**' \
    "${REPO_ROOT}/scripts/" "${REPO_ROOT}/tests/" 2>/dev/null || true)

  # Guard patterns: a line "textually guarding" PANE_TEST_ROOT, either the
  # bash required-var idiom or an early-exit presence check — the two
  # shapes the audit names explicitly.
  local guard_pattern='\$\{PANE_TEST_ROOT:\?|-z[[:space:]]+"?\$\{?PANE_TEST_ROOT\}?"?'

  local violations=""
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    local filepath lineno content
    filepath=$(echo "$line" | cut -d: -f1)
    lineno=$(echo "$line" | cut -d: -f2)
    content=$(echo "$line" | cut -d: -f3-)

    # A guard earlier in the SAME file (line number <= this candidate's)
    # suppresses the violation — "behind the test-root check" per spec.
    local earliest_guard_line
    earliest_guard_line=$(rg -n -e "$guard_pattern" "$filepath" 2>/dev/null | head -1 | cut -d: -f1 || true)

    if [[ -n "$earliest_guard_line" ]] && [[ "$earliest_guard_line" -le "$lineno" ]]; then
      echo "│ ⏭ GUARDED (PANE_TEST_ROOT check at line ${earliest_guard_line}): ${filepath}:${lineno}" >&2
      continue
    fi

    violations="${violations}${filepath}:${lineno}:${content}"$'\n'
  done <<< "$candidates"

  violations=$(echo "$violations" | grep -v '^$' || true)

  run_guard "G8" "$description" "" "$violations"
}

guard_g6() {
  local description="Forbid minProtocol/maxProtocol/protocol integer literals outside named constants in src-gateway/src/"

  # Patterns match JSON-body key literals like "minProtocol": 3 or "protocol": 4.
  # They do NOT match Rust const definitions (pub(crate) const OC_PROTOCOL_MIN: u32 = 3)
  # because those lack the quoted JSON-key form "minProtocol":.
  # Scope: src-gateway/src/ INCLUDING test modules (no *_test*.rs exclusion).
  # Matt directive (#1336): "A test that hardcodes \"protocol\": 3 is a blind spot."
  local rg_output
  rg_output=$(rg --no-heading -n --type rust \
    -e '"minProtocol"\s*:\s*[0-9]' \
    -e '"maxProtocol"\s*:\s*[0-9]' \
    -e '"protocol"\s*:\s*[0-9]' \
    --glob '!target/**' \
    "${REPO_ROOT}/src-gateway/src/" 2>/dev/null || true)

  run_guard "G6" "$description" "" "$rg_output"
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------
COMMAND="${1:-all}"

case "$COMMAND" in
  g1) guard_g1 ;;
  g2) guard_g2 ;;
  g3) guard_g3 ;;
  g4) guard_g4 ;;
  g5) guard_g5 ;;
  g6) guard_g6 ;;
  g7) guard_g7 ;;
  g8) guard_g8 ;;
  all)
    guard_g1
    guard_g2
    guard_g3
    guard_g4
    guard_g5
    guard_g6
    guard_g7
    guard_g8
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    if [[ "$OVERALL_EXIT" -eq 0 ]]; then
      echo "forbid-hardcoded-constants: all guards passed"
    else
      echo "forbid-hardcoded-constants: one or more guards FAILED (see above)"
    fi
    echo "════════════════════════════════════════════════════════════════"
    ;;
  test)
    exec "$(dirname "$0")/tests/run-tests.sh"
    ;;
  *)
    echo "Usage: run-guards.sh [g1|g2|g3|g4|g5|g6|g7|g8|all|test]" >&2
    exit 1
    ;;
esac

exit "$OVERALL_EXIT"
