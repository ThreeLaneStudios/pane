#!/usr/bin/env bash
# =============================================================================
# check-spec-first.sh — spec-first gate decision logic
# =============================================================================
#
# Rule enforced:
#   - Core behavioral files changed → require a docs/spec/ change, OR a
#     verified `spec-covered-by:` (a MERGED PR that touched docs/spec/).
#     `spec-skip:` alone NEVER passes for core behavioral files — see the
#     #604/#605 incident note inline below.
#   - Other source changed (src/ or src-tauri/src/, non-core) → require
#     any docs/ change, OR `spec-skip:`, OR a verified `spec-covered-by:`.
#
# Fixes two bugs in the previous inline workflow logic
# (.github/workflows/spec-first.yml, former lines 43-63):
#
#   Bug 1 — `spec-covered-by:` was only read INSIDE the `spec-skip:` branch,
#   so a code-only PR that correctly cited a merged spec PR failed unless it
#   ALSO carried a dummy `spec-skip:` line, even though the gate's own error
#   message said `spec-covered-by:` alone is enough. Bit #2967, #3002, #3014.
#
#   Bug 2 — the top-level docs check matched ANY `docs/` change
#   (`grep -E '^docs/'`), so a PR touching core behavioral files plus, say,
#   a `docs/audits/` file passed with no spec update at all.
#
# Input contract (so this can be tested without a real PR or network):
#   - stdin:    changed file paths, one per line — same shape as
#               `git diff --name-only origin/main...HEAD`
#   - env PR_BODY: the PR description text (optional; default empty)
#
# Testing seam: the `gh pr view` / `gh pr diff` calls inside verify_covered_by
# below are the seam. Tests prepend a fake `gh` executable to PATH (see
# scripts/tests/check-spec-first/fixtures/gh) that returns canned
# state/diff output for known PR numbers — no network, no real `gh` needed.
#
# Exit codes:
#   0 — spec-first discipline satisfied (or not applicable — no src changed)
#   1 — violation
#
# Usage:
#   git diff --name-only origin/main...HEAD | PR_BODY="$BODY" scripts/check-spec-first.sh
#
# Called by: .github/workflows/spec-first.yml
# Tested by: scripts/tests/check-spec-first/run-tests.sh
# =============================================================================
set -euo pipefail

PR_BODY="${PR_BODY:-}"
CHANGED="$(cat)"

# Core behavioral files, the canonical and only list; there is no
# workflow-side copy to cross-reference (none exists in
# .github/workflows/spec-first.yml). Bare filenames are end-anchored ($)
# so a match requires the whole path component, not a prefix (#3234); the
# three directory prefixes (ai/, sync/, services/) stay unanchored by
# design: anchoring a prefix makes it match nothing.
CORE_BEHAVIORAL_PATTERN='^(src-tauri/src/(ai/|sync/|agents\.rs$|database\.rs$|migrations\.rs$|notes\.rs$|projects\.rs$|system_files\.rs$|sync_client\.rs$|import\.rs$|backup\.rs$|watcher\.rs$)|src/lib/(services/|chat-memory\.ts$|components/Onboarding\.svelte$|components/Settings\.svelte$))'

SRC_CHANGED=$(echo "$CHANGED" | grep -E '^(src/|src-tauri/src/|src-gateway/src/)' || true)
DOCS_CHANGED=$(echo "$CHANGED" | grep -E '^docs/' || true)
SPEC_UPDATED=$(echo "$CHANGED" | grep -E '^docs/spec/' || true)
CORE_BEHAVIORAL=$(echo "$CHANGED" | grep -E "$CORE_BEHAVIORAL_PATTERN" || true)

SPEC_SKIP=$(echo "$PR_BODY" | grep -iE '^[[:space:]]*spec-skip:' || true)
SPEC_COVERED_LINE=$(echo "$PR_BODY" | grep -iE '^[[:space:]]*spec-covered-by:' || true)
SPEC_COVERED_NUM=$(echo "$SPEC_COVERED_LINE" | grep -oE '#[0-9]+' | head -1 | tr -d '#' || true)

# ---------------------------------------------------------------------------
# verify_covered_by <pr_number> — unchanged verification semantics from the
# original inline gate: the referenced PR must exist, be MERGED, and have
# touched docs/spec/. Returns 0 if verified, 1 otherwise. Prints the same
# ✅/❌ lines the inline version did (CI log output is unchanged).
# ---------------------------------------------------------------------------
verify_covered_by() {
  local num="$1"
  local ref_state ref_spec_files

  ref_state=$(gh pr view "$num" --json state -q .state 2>/dev/null || echo "NOT_FOUND")

  if [ "$ref_state" = "NOT_FOUND" ]; then
    echo "❌ spec-covered-by: #$num — PR not found"
    return 1
  elif [ "$ref_state" != "MERGED" ]; then
    echo "❌ spec-covered-by: #$num — PR not merged (state: $ref_state)"
    return 1
  fi

  ref_spec_files=$(gh pr diff "$num" --name-only 2>/dev/null | grep -E '^docs/spec/' || true)
  if [ -z "$ref_spec_files" ]; then
    echo "❌ spec-covered-by: #$num — referenced PR did not touch docs/spec/"
    return 1
  fi

  echo "✅ spec-covered-by: #$num verified (merged, touched docs/spec/)"
  return 0
}

# ---------------------------------------------------------------------------
# No source changes at all — not applicable.
# ---------------------------------------------------------------------------
if [ -z "$SRC_CHANGED" ]; then
  echo "✅ No source changes — spec-first check not applicable"
  exit 0
fi

# ---------------------------------------------------------------------------
# Core behavioral files changed — the strict path.
# ---------------------------------------------------------------------------
if [ -n "$CORE_BEHAVIORAL" ]; then
  if [ -n "$SPEC_UPDATED" ]; then
    echo "✅ Core behavioral files changed — docs/spec/ update present"
    exit 0
  fi

  if [ -n "$SPEC_COVERED_NUM" ]; then
    if verify_covered_by "$SPEC_COVERED_NUM"; then
      exit 0
    else
      exit 1
    fi
  fi

  # Why This Exists (spec-skip restriction): On 2026-04-21, PRs #604 and
  # #605 shipped behavioral changes (reranker degradation, system prompt
  # daily log) with spec-skip — skipping spec updates to ship faster.
  # Path-based restriction prevents this: core behavioral files cannot use
  # spec-skip without also updating docs/spec/ (or citing a merged spec PR
  # via spec-covered-by:). Incidents: #604, #605. Issues: #611.
  echo "❌ SPEC-FIRST GATE: core behavioral files changed without docs/spec/ update."
  echo ""
  echo "Changed core files:"
  echo "$CORE_BEHAVIORAL"
  echo ""
  if [ -n "$SPEC_SKIP" ]; then
    echo "Note: 'spec-skip:' alone never satisfies this gate for core behavioral"
    echo "files (see #604/#605 incident note) — a docs/spec/ update or a verified"
    echo "'spec-covered-by:' is required instead."
    echo ""
  fi
  echo "Remediation paths:"
  echo "  1. Bug fix aligning code to existing spec?"
  echo "     → Add status marker to relevant docs/spec/*.md in this PR"
  echo "     (e.g., \"> Status: BUILT (vX.Y.Z, PR #NNNN) — [description]\")"
  echo ""
  echo "  2. Spec already landed in a prior PR?"
  echo "     → Add 'spec-covered-by: #NNNN [justification]' to PR description"
  echo "     Referenced PR must be: merged + touched docs/spec/"
  echo ""
  echo "  3. New behavioral change?"
  echo "     → Spec PR first (separate PR to docs/spec/), merge it, then rebase this PR"
  echo ""
  echo "See docs/dev-hygiene-playbook.md §2."
  exit 1
fi

# ---------------------------------------------------------------------------
# Other source changed (non-core) — the relaxed path: any docs/, spec-skip,
# or a verified spec-covered-by: all satisfy it.
# ---------------------------------------------------------------------------
if [ -n "$DOCS_CHANGED" ]; then
  echo "✅ Source changed with docs update — spec-first discipline maintained"
  exit 0
fi

if [ -n "$SPEC_SKIP" ]; then
  echo "⏭️ Spec-first check skipped: $SPEC_SKIP"
  exit 0
fi

if [ -n "$SPEC_COVERED_NUM" ]; then
  if verify_covered_by "$SPEC_COVERED_NUM"; then
    exit 0
  else
    exit 1
  fi
fi

echo "❌ SPEC-FIRST VIOLATION"
echo ""
echo "Source files changed without a corresponding docs/ update:"
echo "$SRC_CHANGED" | head -20
echo ""
echo "Either:"
echo "  1. Update the relevant spec/doc file in this PR"
echo "  2. Add 'spec-skip: [reason]' to the PR description"
echo "  3. Add 'spec-covered-by: #NNNN [justification]' to the PR description"
echo "     (referenced PR must be merged + touched docs/spec/)"
echo ""
exit 1
