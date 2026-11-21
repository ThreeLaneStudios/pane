#!/bin/bash
set -e
cd "$(dirname "$0")/.."

# === ARCHITECTURE INTEGRITY CHECKS ===
echo "🔍 Running architecture integrity checks..."

# Check 1: No "Pane System" as a project in new code
PANE_SYSTEM_HITS=$(grep -rn '"Pane System"' src-tauri/src/ --include="*.rs" | grep -v "// " | grep -v "migration" | grep -v "DELETE" | grep -v "#\[test\]" | grep -v "mod tests" | grep -v "fn test_" | grep -v "_tests\.rs" || true)
if [ -n "$PANE_SYSTEM_HITS" ]; then
    echo "❌ INVARIANT VIOLATION: 'Pane System' found as a project reference in code"
    echo "   §24: Agents use ai_agents table, not projects. See docs/INVARIANTS.md"
    echo "$PANE_SYSTEM_HITS"
    exit 1
fi

# Check 2: No agent data written to projects table  
AGENT_PROJECT_HITS=$(grep -rn 'INSERT INTO projects.*agent\|INSERT INTO projects.*system\|is_system.*=.*1' src-tauri/src/ --include="*.rs" | grep -v "migration" | grep -v "// legacy" || true)
if [ -n "$AGENT_PROJECT_HITS" ]; then
    echo "⚠️  WARNING: Possible agent data being written to projects table"
    echo "   §24: Agents belong in ai_agents table only. See docs/INVARIANTS.md"
    echo "$AGENT_PROJECT_HITS"
fi

# Check 3: verify_system_notes_integrity should not be called unconditionally
SYSTEM_NOTES_HITS=$(grep -rn 'verify_system_notes_integrity' src-tauri/src/ --include="*.rs" | grep -v 'legacy-ok' | grep -v 'legacy' | grep -v '_tests\.rs' | grep -v 'fn test_' || true)
if [ -n "$SYSTEM_NOTES_HITS" ]; then
    echo "❌ INVARIANT VIOLATION: verify_system_notes_integrity() called without ai_agents table check"
    echo "   §24.13: Only verify_agents_integrity() should run when ai_agents exists"
    echo "$SYSTEM_NOTES_HITS"
    exit 1
fi

# Print invariants reminder
echo ""
echo "📋 Architecture invariants (docs/INVARIANTS.md):"
cat docs/INVARIANTS.md 2>/dev/null | grep "^- NEVER" || echo "   (INVARIANTS.md not found — check docs/)"
echo ""
echo "✅ Architecture integrity checks passed"
echo ""

# ── Version Duplicate Check ───────────────────────────────────────────────
# Prevent building the same version twice. Every build must bump the version.
CURRENT_VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
BUILDS_DIR="$HOME/Shared/Pane/builds"

if [ -f "$BUILDS_DIR/Pane-${CURRENT_VERSION}.dmg" ]; then
  echo ""
  echo "❌ Version ${CURRENT_VERSION} already exists at $BUILDS_DIR/Pane-${CURRENT_VERSION}.dmg"
  echo "   Bump the version in src-tauri/tauri.conf.json AND src-tauri/Cargo.toml before building."
  echo "   Example: 0.35.1 → 0.35.2"
  echo ""
  if [ "$FORCE_VERSION_OVERWRITE" != "1" ]; then
    exit 1
  else
    echo "⚠️  FORCE_VERSION_OVERWRITE=1 set — overwriting existing build"
  fi
fi
echo "✅ Version ${CURRENT_VERSION} is new (no existing DMG)"
echo ""

# ── Spec-Driven Development Checks ────────────────────────────────────────
echo "📋 Running spec-driven development checks..."

# Check A: Spec update required for code changes
STAGED_CODE_FILES=$(git diff --cached --name-only | grep -E '^(src/|src-tauri/src/)' || true)
STAGED_DOCS_FILES=$(git diff --cached --name-only | grep -E '^docs/' || true)

if [ -n "$STAGED_CODE_FILES" ] && [ -z "$STAGED_DOCS_FILES" ]; then
  if [ "$SPEC_SKIP_APPROVED_BY" = "matt" ]; then
    echo "⚠️  Spec check skipped (approved by Matt)"
  else
    echo ""
    echo "❌ Code changes detected without docs/ update. Update the spec."
    echo "   Set SPEC_SKIP_APPROVED_BY=matt to bypass (requires Matt's explicit approval)."
    echo ""
    echo "   Staged code files:"
    echo "$STAGED_CODE_FILES" | sed 's/^/     - /'
    echo ""
    exit 1
  fi
fi

# Check B: Issue reference in commit message
COMMIT_MSG=$(git log -1 --format=%s 2>/dev/null || echo "")
if ! echo "$COMMIT_MSG" | grep -qE '#[0-9]+'; then
  echo "⚠️  No issue reference found in commit message. Expected #NNN."
  echo "   Current message: $COMMIT_MSG"
fi

echo "✅ Spec-driven development checks passed"
echo ""

# ── Load Apple signing credentials ────────────────────────────────────────
# Source .env and export Apple signing variables for Tauri code signing
if [ -f ".env" ]; then
  echo "🔑 Loading Apple signing credentials from .env..."
  set -a  # automatically export all variables
  source .env
  set +a
  
  # Verify critical signing variables are set
  if [ -n "$APPLE_SIGNING_IDENTITY" ] && [ -n "$APPLE_ID" ] && [ -n "$APPLE_PASSWORD" ] && [ -n "$APPLE_TEAM_ID" ]; then
    echo "✅ Apple signing credentials loaded"
    
    # Sign native libraries before Tauri bundles them
    ./scripts/sign-dylibs.sh
  else
    echo "⚠️  Warning: Some Apple signing variables are missing from .env"
  fi
else
  echo "⚠️  Warning: .env file not found - build will be ad-hoc signed"
fi
echo ""

echo "🧪 Running svelte-check..."
npx svelte-check --threshold error
if [ $? -ne 0 ]; then
  echo "❌ svelte-check failed. Fix TypeScript/Svelte errors before building."
  exit 1
fi
echo "✅ svelte-check passed"
echo ""
