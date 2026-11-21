#!/usr/bin/env bash
# =============================================================================
# check-release-source.sh — verify a tag's commit has been merged to main
# =============================================================================
# A DMG was once accidentally built from an unmerged branch and shipped as
# v1.0.46-dev. This script closes the manual `gh release create v*-dev
# --prerelease` path: it confirms the tag's commit is actually on
# origin/main before a dev release is published.
#
# The CI `release-publish.yml` workflow already has `guard-tag-branch` for
# tag-triggered pipeline releases. This script covers the manual path.
#
# Usage:
#   bash scripts/check-release-source.sh v1.0.46-dev
#
# Exit codes:
#   0 — tag's commit is an ancestor of origin/main, release permitted
#   1 — tag's commit is NOT on origin/main, release blocked (or usage error)
# =============================================================================
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: $0 <tag-name>" >&2
  exit 1
fi

TAG="$1"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

git fetch origin main --quiet || true

if ! SHA=$(git rev-list -n 1 "$TAG" 2>/dev/null); then
  echo "❌ ERROR: tag '$TAG' not found" >&2
  exit 1
fi

if git merge-base --is-ancestor "$SHA" origin/main; then
  echo "✅ Tag $TAG ($SHA) is on main — release permitted"
  exit 0
else
  echo "❌ Tag $TAG ($SHA) is NOT on main — release blocked. Only tag commits that have been merged to main."
  exit 1
fi
