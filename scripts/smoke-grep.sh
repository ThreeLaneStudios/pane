#!/usr/bin/env bash
# Grep stderr for severity patterns
# Usage: smoke-grep.sh <log-file>
#
# Exits 0 if no violations found (excluding allowlisted)
# Exits 1 if violations found
#
# Severity patterns checked:
#   [CRITICAL]
#   [FATAL]
#   panicked at
#   thread '.*' panicked
#   RUST_BACKTRACE=
#
# Allowlist applied via scripts/extract-smoke-allowlist.sh

set -euo pipefail

LOG="${1:-}"
if [ -z "$LOG" ] || [ ! -f "$LOG" ]; then
  echo "Usage: smoke-grep.sh <log-file>"
  exit 2
fi

PATTERNS=(
  '\[CRITICAL\]'
  '\[FATAL\]'
  'panicked at'
  "thread '[^']*' panicked"
  'RUST_BACKTRACE='
)

# Extract allowlist annotations from source
ALLOWLIST_FILE=$(mktemp)
trap "rm -f '$ALLOWLIST_FILE'" EXIT

echo "[SMOKE-GREP] Extracting allowlist annotations..." >&2
if ! ./scripts/extract-smoke-allowlist.sh > "$ALLOWLIST_FILE" 2>&1; then
  echo "[SMOKE-GREP] ❌ Allowlist extraction failed:" >&2
  cat "$ALLOWLIST_FILE" >&2
  exit 1
fi

# Count allowlist entries
ALLOWLIST_COUNT=$(grep -c . "$ALLOWLIST_FILE" || true)
echo "[SMOKE-GREP] Found $ALLOWLIST_COUNT allowlist entries" >&2

VIOLATIONS=0
ALLOWED_COUNT=0

echo "[SMOKE-GREP] Checking severity patterns against log: $LOG" >&2

for PATTERN in "${PATTERNS[@]}"; do
  # Find all matches for this pattern
  while IFS= read -r LINE; do
    # Check against allowlist
    ALLOWED=0
    
    if [ "$ALLOWLIST_COUNT" -gt 0 ]; then
      while IFS= read -r ALLOWED_REGEX; do
        if [ -n "$ALLOWED_REGEX" ] && echo "$LINE" | grep -qE "$ALLOWED_REGEX"; then
          ALLOWED=1
          echo "[SMOKE-GREP] ✓ Allowed (matches allowlist): $LINE" >&2
          ALLOWED_COUNT=$((ALLOWED_COUNT + 1))
          break
        fi
      done < "$ALLOWLIST_FILE"
    fi
    
    if [ "$ALLOWED" -eq 0 ]; then
      echo "❌ Severity violation: $LINE"
      VIOLATIONS=$((VIOLATIONS + 1))
    fi
  done < <(grep -E "$PATTERN" "$LOG" 2>/dev/null || true)
done

echo "" >&2
echo "[SMOKE-GREP] Summary:" >&2
echo "[SMOKE-GREP]   Violations: $VIOLATIONS" >&2
echo "[SMOKE-GREP]   Allowlisted: $ALLOWED_COUNT" >&2

if [ "$VIOLATIONS" -gt 0 ]; then
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "❌ Found $VIOLATIONS severity violation(s) in startup log"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo ""
  echo "If a violation is expected, add an allowlist annotation above the"
  echo "emitting line in source code:"
  echo ""
  echo "  // §smoke-allowlist: <regex matching the line> — <justification>"
  echo "  eprintln!(\"[CRITICAL] Expected message\");"
  echo ""
  echo "Regex must:"
  echo "  - Contain at least 4 consecutive literal characters (be specific)"
  echo "  - NOT be trivial (e.g., .*, .+, .)"
  echo "  - Include a justification after the em-dash (—)"
  echo ""
  exit 1
fi

echo "[SMOKE-GREP] ✓ No severity violations found" >&2
exit 0
