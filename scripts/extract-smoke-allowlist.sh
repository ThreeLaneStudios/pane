#!/usr/bin/env bash
# Extract smoke-allowlist annotations from source
# Format: // §smoke-allowlist: <regex> — <justification>
#
# Validation rules (per ADR-0018 acceptance criteria #11-15):
# - Empty regexes → error
# - Trivial regexes (.*, .+, .) → error
# - Regex must contain at least 4 consecutive non-metacharacter literal chars
# - Empty justification → error
# - Malformed regex (uncompilable in grep -E) → error
# - Annotation not preceding an emission call → warning (not error)
#
# Output: One regex per line on stdout
# Exit codes: 0 (success), 1 (validation errors found)

set -euo pipefail

SOURCE_DIRS=("src-tauri/src" "src")

# Temp files for tracking
ERRORS_TMP=$(mktemp)
WARNINGS_TMP=$(mktemp)
REGEXES_TMP=$(mktemp)
trap "rm -f '$ERRORS_TMP' '$WARNINGS_TMP' '$REGEXES_TMP'" EXIT

# Function to validate and extract allowlist from a single file
process_file() {
  local FILE="$1"
  
  # Get line numbers of allowlist annotations
  local LINE_NUMS=$(grep -n "§smoke-allowlist:" "$FILE" | cut -d: -f1 || true)
  
  if [ -z "$LINE_NUMS" ]; then
    return
  fi
  
  while IFS= read -r LINE_NUM; do
    local LINE_TEXT=$(sed -n "${LINE_NUM}p" "$FILE")
    
    # Extract regex (between colon and em-dash)
    local REGEX=$(echo "$LINE_TEXT" | sed -n 's/.*§smoke-allowlist: *\([^—]*\) *—.*/\1/p' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    
    # Extract justification (after em-dash)
    local JUSTIFICATION=$(echo "$LINE_TEXT" | sed -n 's/.*— *\(.*\)/\1/p' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    
    # Validation: Empty regex
    if [ -z "$REGEX" ]; then
      echo "ERROR: ${FILE}:${LINE_NUM}: Empty regex in smoke-allowlist annotation" >> "$ERRORS_TMP"
      continue
    fi
    
    # Validation: Trivial regexes
    if [ "$REGEX" = ".*" ] || [ "$REGEX" = ".+" ] || [ "$REGEX" = "." ]; then
      echo "ERROR: ${FILE}:${LINE_NUM}: Trivial regex '$REGEX' not allowed (must be specific)" >> "$ERRORS_TMP"
      continue
    fi
    
    # Validation: Must contain at least 4 consecutive literal characters
    # Remove common metacharacters and count remaining consecutive chars
    local LITERAL_TEST=$(echo "$REGEX" | sed 's/[.^$*+?{}()|[\\]//g')
    local MAX_CONSECUTIVE=0
    local CURRENT_CONSECUTIVE=0
    for ((i=0; i<${#LITERAL_TEST}; i++)); do
      local CHAR="${LITERAL_TEST:$i:1}"
      if [ -n "$CHAR" ] && [ "$CHAR" != " " ]; then
        CURRENT_CONSECUTIVE=$((CURRENT_CONSECUTIVE + 1))
        if [ "$CURRENT_CONSECUTIVE" -gt "$MAX_CONSECUTIVE" ]; then
          MAX_CONSECUTIVE=$CURRENT_CONSECUTIVE
        fi
      else
        CURRENT_CONSECUTIVE=0
      fi
    done
    
    if [ "$MAX_CONSECUTIVE" -lt 4 ]; then
      echo "ERROR: ${FILE}:${LINE_NUM}: Regex must contain at least 4 consecutive literal characters (got $MAX_CONSECUTIVE from '$REGEX')" >> "$ERRORS_TMP"
      continue
    fi
    
    # Validation: Empty justification
    if [ -z "$JUSTIFICATION" ]; then
      echo "ERROR: ${FILE}:${LINE_NUM}: Empty justification in smoke-allowlist annotation" >> "$ERRORS_TMP"
      continue
    fi
    
    # Validation: Malformed regex (test with grep -E)
    # Test by attempting to compile it (use a non-empty test string)
    if ! echo "test" | grep -E "$REGEX" >/dev/null 2>&1 && ! echo "test" | grep -vE "$REGEX" >/dev/null 2>&1; then
      echo "ERROR: ${FILE}:${LINE_NUM}: Malformed regex '$REGEX' (not valid for grep -E)" >> "$ERRORS_TMP"
      continue
    fi
    
    # Check if annotation precedes an emission call
    # Look at the next non-empty, non-comment line
    local NEXT_LINE_NUM=$((LINE_NUM + 1))
    local NEXT_LINE=$(sed -n "${NEXT_LINE_NUM}p" "$FILE" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    
    # Skip empty lines and comments to find the actual next statement
    local MAX_LOOK_AHEAD=5
    local LOOK_COUNT=0
    while [ $LOOK_COUNT -lt $MAX_LOOK_AHEAD ] && [ -n "$NEXT_LINE" ] && ( [ -z "$NEXT_LINE" ] || echo "$NEXT_LINE" | grep -q '^\(//\|/\*\)' ); do
      NEXT_LINE_NUM=$((NEXT_LINE_NUM + 1))
      NEXT_LINE=$(sed -n "${NEXT_LINE_NUM}p" "$FILE" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
      LOOK_COUNT=$((LOOK_COUNT + 1))
    done
    
    # Check if it's an emission call
    if [ -n "$NEXT_LINE" ] && ! echo "$NEXT_LINE" | grep -qE '(eprintln!|println!|log::error!|log::warn!|console\.(error|warn|log))'; then
      echo "WARNING: ${FILE}:${LINE_NUM}: Allowlist annotation does not precede an emission call" >> "$WARNINGS_TMP"
    fi
    
    # Output the regex
    echo "$REGEX" >> "$REGEXES_TMP"
  done <<< "$LINE_NUMS"
}

# Find all source files with allowlist annotations
for SOURCE_DIR in "${SOURCE_DIRS[@]}"; do
  if [ ! -d "$SOURCE_DIR" ]; then
    continue
  fi
  
  # Find all Rust, TypeScript, JavaScript, Svelte files with allowlist annotations
  while IFS= read -r FILE; do
    if [ -n "$FILE" ]; then
      process_file "$FILE"
    fi
  done < <(find "$SOURCE_DIR" -type f \( -name "*.rs" -o -name "*.ts" -o -name "*.js" -o -name "*.svelte" \) -exec grep -l "§smoke-allowlist:" {} \; 2>/dev/null || true)
done

# Output regexes to stdout
if [ -s "$REGEXES_TMP" ]; then
  cat "$REGEXES_TMP"
fi

# Report errors and warnings to stderr
ERROR_COUNT=$(wc -l < "$ERRORS_TMP" | tr -d ' ')
WARN_COUNT=$(wc -l < "$WARNINGS_TMP" | tr -d ' ')

if [ "$ERROR_COUNT" -gt 0 ]; then
  cat "$ERRORS_TMP" >&2
  echo "" >&2
  echo "❌ Found $ERROR_COUNT allowlist validation error(s)" >&2
  if [ "$WARN_COUNT" -gt 0 ]; then
    cat "$WARNINGS_TMP" >&2
    echo "⚠️  Found $WARN_COUNT warning(s)" >&2
  fi
  exit 1
fi

if [ "$WARN_COUNT" -gt 0 ]; then
  cat "$WARNINGS_TMP" >&2
  echo "⚠️  Found $WARN_COUNT warning(s) (not blocking)" >&2
fi

exit 0
