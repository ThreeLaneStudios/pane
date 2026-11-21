#!/usr/bin/env bash
# =============================================================================
# check-spec-first.sh unit tests
# =============================================================================
# Seven required cases (see .github/workflows/spec-first.yml / PR description
# for the full rule) plus a self-check sanity case. Each test feeds a
# synthetic changed-file list (stdin) and PR body (env PR_BODY) straight into
# the production script — no real PR, no real network. The `gh pr view`/
# `gh pr diff` calls are satisfied by fixtures/gh, a fake `gh` prepended onto
# PATH for the duration of this run only.
#
# Run: scripts/tests/check-spec-first/run-tests.sh
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SCRIPT="$REPO_ROOT/scripts/check-spec-first.sh"
FIXTURES="$SCRIPT_DIR/fixtures"

export PATH="$FIXTURES:$PATH"

PASS=0
FAIL=0

# ---------------------------------------------------------------------------
# run_case <name> <expected_exit> <changed_files> <pr_body> [fake_gh_state] [fake_gh_spec_files]
# ---------------------------------------------------------------------------
run_case() {
  local name="$1" expected_exit="$2" changed="$3" pr_body="$4"
  local fake_state="${5:-}" fake_spec_files="${6:-}"

  local output exit_code
  output=$(printf '%s\n' "$changed" | PR_BODY="$pr_body" FAKE_GH_STATE="$fake_state" FAKE_GH_SPEC_FILES="$fake_spec_files" "$SCRIPT" 2>&1)
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

echo "Running check-spec-first.sh test suite..."
echo ""

# 1. core files + valid spec-covered-by + no spec-skip → passes
run_case \
  "case1: core file + valid spec-covered-by, no spec-skip -> PASS" \
  0 \
  "src-tauri/src/notes.rs" \
  "Relates to #9999
spec-covered-by: #3012" \
  "MERGED" "docs/spec/data-refresh.md"

# 2. core files + spec-skip only → fails
run_case \
  "case2: core file + spec-skip only -> FAIL" \
  1 \
  "src-tauri/src/notes.rs" \
  "spec-skip: shipping fast, no spec needed"

# 3. core files + a non-spec docs/ change only → fails
run_case \
  "case3: core file + non-spec docs/ change only -> FAIL" \
  1 \
  "src-tauri/src/notes.rs
docs/audits/2026-10-11-unrelated-audit.md" \
  ""

# 4. core files + a docs/spec/ change → passes
run_case \
  "case4: core file + docs/spec/ change -> PASS" \
  0 \
  "src-tauri/src/notes.rs
docs/spec/core-data-model.md" \
  ""

# 5. other source + spec-skip → passes
run_case \
  "case5: non-core src + spec-skip -> PASS" \
  0 \
  "src-tauri/src/release.rs" \
  "spec-skip: trivial refactor, no behavior change"

# 6. other source, no docs, no markers → fails
run_case \
  "case6: non-core src, no docs, no markers -> FAIL" \
  1 \
  "src-tauri/src/release.rs" \
  ""

# 7a. spec-covered-by pointing at an UNMERGED PR → fails
run_case \
  "case7a: spec-covered-by points at unmerged PR -> FAIL" \
  1 \
  "src-tauri/src/notes.rs" \
  "spec-covered-by: #4000" \
  "OPEN" ""

# 7b. spec-covered-by pointing at a PR that did NOT touch docs/spec/ → fails
run_case \
  "case7b: spec-covered-by points at PR that didn't touch docs/spec/ -> FAIL" \
  1 \
  "src-tauri/src/notes.rs" \
  "spec-covered-by: #4001" \
  "MERGED" "docs/audits/unrelated.md"

# 8. database.rs.before_composite_pk + spec-skip -> PASS (regression, #3234:
# the bare-filename entries in CORE_BEHAVIORAL_PATTERN are now end-anchored,
# so this backup-snapshot filename, which only shares database.rs as a
# PREFIX and not the whole path component, no longer matches core behavioral.
# It falls through to the relaxed non-core path instead, where spec-skip
# alone satisfies the gate. Before the anchor fix, this filename matched
# CORE_BEHAVIORAL unanchored, landing on the strict path where spec-skip
# alone never passes (the #604/#605 restriction above), so this case would
# FAIL (exit 1) against the unfixed pattern.
run_case \
  "case8: database.rs.before_composite_pk + spec-skip -> PASS (regression #3234: end-anchored bare filenames)" \
  0 \
  "src-tauri/src/database.rs.before_composite_pk" \
  "spec-skip: dead backup snapshot, no behavior change"

# Bonus sanity case: a CI-only PR (this PR's own shape — .github/ + scripts/,
# no src/) must not trigger the SRC_CHANGED branch at all.
run_case \
  "bonus: CI-only change (no src/) -> PASS (not applicable)" \
  0 \
  ".github/workflows/spec-first.yml
scripts/check-spec-first.sh" \
  ""

echo ""
echo "--- Real workflow wiring ---"
echo ""
echo "The cases above call scripts/check-spec-first.sh directly with PR_BODY"
echo "exported per-invocation (PR_BODY=\"...\" SCRIPT), which is NOT how the"
echo "real workflow wires the two files together — .github/workflows/spec-first.yml"
echo "assigns PR_BODY in a separate statement, then runs the script as a"
echo "subprocess. The cases below extract the actual run: block out of the"
echo "live workflow YAML (scripts/tests/check-spec-first/extract-run-block.sh —"
echo "marker-sliced from the file on disk, never hand-copied) and execute it"
echo "exactly as GitHub Actions does: bash --noprofile --norc -eo pipefail,"
echo "env wiped (env -i) except PATH/PR_BODY_ENV/GH_TOKEN plus this test's"
echo "FAKE_CHANGED/FAKE_GH_* controls for the git/gh fakes on PATH."
echo ""

WORKFLOW_FILE="$REPO_ROOT/.github/workflows/spec-first.yml"
EXTRACT="$SCRIPT_DIR/extract-run-block.sh"
STEP_NAME="Check spec-first discipline"

# run_wiring_case <name> <expected_exit> <fake_changed_files> <pr_body> [fake_gh_state] [fake_gh_spec_files]
run_wiring_case() {
  local name="$1" expected_exit="$2" changed="$3" pr_body="$4"
  local fake_state="${5:-}" fake_spec_files="${6:-}"

  local run_block_file output exit_code
  run_block_file="$(mktemp)"
  if ! "$EXTRACT" "$WORKFLOW_FILE" "$STEP_NAME" > "$run_block_file" 2>/tmp/extract-run-block.err; then
    echo "  ❌ FAIL: ${name} (could not extract run: block — $(cat /tmp/extract-run-block.err))"
    ((FAIL++)) || true
    rm -f "$run_block_file"
    return
  fi

  output=$(cd "$REPO_ROOT" && env -i \
    PATH="$FIXTURES:/usr/bin:/bin:/usr/local/bin" \
    PR_BODY_ENV="$pr_body" \
    GH_TOKEN="fake" \
    FAKE_CHANGED="$changed" \
    FAKE_GH_STATE="$fake_state" \
    FAKE_GH_SPEC_FILES="$fake_spec_files" \
    bash --noprofile --norc -eo pipefail "$run_block_file" 2>&1)
  exit_code=$?
  rm -f "$run_block_file"

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

# wiring1: core file + valid spec-covered-by, no spec-skip -> PASS, driven
# through the REAL workflow run: block. This is the exact shape that failed
# in production before `export PR_BODY="$PR_BODY_ENV"` — PR_BODY was never
# inherited by the scripts/check-spec-first.sh subprocess without it.
run_wiring_case \
  "wiring1: real run: block, core file + valid spec-covered-by, no spec-skip -> PASS" \
  0 \
  "src-tauri/src/notes.rs
CHANGELOG.md" \
  "Relates to #9999
spec-covered-by: #3012" \
  "MERGED" "docs/spec/data-refresh.md"

# wiring2: non-core file + spec-skip -> PASS, through the real run: block.
run_wiring_case \
  "wiring2: real run: block, non-core file + spec-skip -> PASS" \
  0 \
  "src-tauri/src/release.rs
CHANGELOG.md" \
  "spec-skip: trivial refactor, no behavior change"

# wiring3: core file + spec-skip only (no docs/spec/, no verified
# spec-covered-by) -> FAIL, through the real run: block. The #604/#605
# incident protection must survive the real wiring, not just the extracted
# script in isolation.
run_wiring_case \
  "wiring3: real run: block, core file + spec-skip only -> FAIL" \
  1 \
  "src-tauri/src/notes.rs
CHANGELOG.md" \
  "spec-skip: shipping fast, no spec needed"

echo ""
echo "============================================================"
echo "Results: ${PASS} passed, ${FAIL} failed"
echo "============================================================"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
exit 0
