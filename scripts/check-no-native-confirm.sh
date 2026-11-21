#!/usr/bin/env bash
# =============================================================================
# check-no-native-confirm.sh — forbid native confirm( in src/
# =============================================================================
#
# Rule enforced:
#   No call to the native, browser-provided `confirm(...)` (bare or
#   `window.confirm(...)`) anywhere under `src/` (frontend source).
#
# Why this exists (#3044 item 6): the pinned Tauri dialog plugin this app
# ships has NO `confirm` command at all — native `confirm()` never asks the
# user anything in Tauri's WebView, it just returns immediately (observed:
# `undefined`/falsy) and the calling code carries on as if the user said no
# (or, depending on the call's own `!confirm(...)` polarity, said yes) —
# silently, every time. This is not a style inconsistency; it is a dialog
# that never fires. Three call sites (`TaskEditModal.svelte`,
# `TaskFullPage.svelte`, `TaskList.svelte`) were still using it when this
# check was added — see CHANGELOG.md for the fix landed in the same commit.
# `alert()` is explicitly NOT forbidden here — per Matt's ruling it already
# works correctly in this WebView; only `confirm()` is broken.
#
# WHAT'S EXCLUDED, and why:
#   - `*.test.ts`/`*.spec.ts` — test files, not shipped behavior.
#   - `//` line comments and `/* */` block comments — comment TEXT
#     mentioning `confirm(` (there are many, documenting past fixes) must
#     never be mistaken for a real call.
#   - A file that declares its OWN local `confirm` (e.g.
#     `function confirm() { ... }`, `const confirm = (...) => ...`),
#     shadowing the global — `AgentPickerPopover.svelte` does this
#     intentionally; a locally-shadowed `confirm()` call in that same file
#     is not the native dialog. Scoped per-file, not a blanket exemption:
#     another file with no such declaration still gets flagged normally.
#
# Input contract (so this can be tested without touching the real tree):
#   arg1 (optional): root directory to scan. Defaults to `src/` (repo-relative,
#   resolved against $REPO_ROOT if set, else the script's own repo root).
#
# Exit codes:
#   0 — no native confirm( found
#   1 — violation(s) found, printed to stdout
#
# Usage:
#   scripts/check-no-native-confirm.sh [root_dir]
#
# Tested by: scripts/tests/check-no-native-confirm/run-tests.sh
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
TARGET_DIR="${1:-$REPO_ROOT/src}"

if [ ! -d "$TARGET_DIR" ]; then
  echo "✅ No such directory ($TARGET_DIR) — nothing to scan"
  exit 0
fi

# Strips `//` line comments, `/* */` block comments, and `<!-- -->` HTML/
# Svelte template comments from a file's content (stdin -> stdout),
# preserving line structure, so comment TEXT (there is a lot of it in this
# repo documenting past confirm()-related fixes) can never be mistaken for
# a real call. Deliberately does NOT attempt to track string/template-
# literal quoting: a .svelte file freely mixes JS (where `'`/`"`/`` ` ``
# really do delimit strings) with plain HTML text (where an apostrophe —
# e.g. "Pane Gateway's terminal" in NewProviderModal.svelte's real markup —
# is just a character, not a string delimiter). A universal quote-tracker
# that doesn't know which mode it's in misfires on exactly that shape,
# treating everything after a stray text apostrophe as "inside a string"
# and silently skipping real `<!-- -->` comments later in the same file —
# confirmed empirically against this exact file while building this
# script. Pure comment-delimiter stripping has no such failure mode and is
# sufficient here: the goal is zero false positives from documentation
# comments, not perfect precision on `confirm(` written out inside a JS
# string literal (not a real call either way, and not observed anywhere
# in this codebase).
strip_comments() {
  python3 - "$1" <<'PYEOF'
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8", errors="replace") as f:
    source = f.read()
out = []
i = 0
n = len(source)
while i < n:
    c = source[i]
    if c == '/' and i + 1 < n and source[i + 1] == '/':
        while i < n and source[i] != '\n':
            out.append(' ')
            i += 1
    elif c == '/' and i + 1 < n and source[i + 1] == '*':
        out.append(' ')
        out.append(' ')
        i += 2
        while i < n and not (source[i] == '*' and i + 1 < n and source[i + 1] == '/'):
            out.append('\n' if source[i] == '\n' else ' ')
            i += 1
        if i < n:
            out.append(' ')
            out.append(' ')
            i += 2
    elif c == '<' and source[i:i + 4] == '<!--':
        end = source.find('-->', i + 4)
        end = (end + 3) if end != -1 else n
        for j in range(i, end):
            out.append('\n' if source[j] == '\n' else ' ')
        i = end
    else:
        out.append(c)
        i += 1
sys.stdout.write(''.join(out))
PYEOF
}

VIOLATIONS=""

while IFS= read -r -d '' file; do
  case "$file" in
    *.test.ts|*.spec.ts|*.test.svelte|*.spec.svelte) continue ;;
  esac

  code_only="$(strip_comments "$file")"

  # File-level shadow exclusion: a local declaration of `confirm` means any
  # `confirm(` call in THIS file is not the native global.
  if printf '%s' "$code_only" | grep -qE '(^|[^.[:alnum:]_])function[[:space:]]+confirm[[:space:]]*\(|(^|[^.[:alnum:]_])(const|let|var)[[:space:]]+confirm[[:space:]]*='; then
    continue
  fi

  matches="$(printf '%s' "$code_only" | grep -nE '(^|[^.[:alnum:]_])(window\.)?confirm[[:space:]]*\(' || true)"
  if [ -n "$matches" ]; then
    while IFS= read -r line; do
      [ -z "$line" ] && continue
      lineno="${line%%:*}"
      VIOLATIONS="${VIOLATIONS}${file}:${lineno}
"
    done <<< "$matches"
  fi
done < <(find "$TARGET_DIR" -type f \( -name '*.svelte' -o -name '*.ts' \) -print0)

if [ -n "$VIOLATIONS" ]; then
  echo "❌ NATIVE confirm( FOUND in $TARGET_DIR"
  echo ""
  echo "The pinned Tauri dialog plugin has no confirm command — native"
  echo "confirm() never asks the user anything in this app's WebView; it"
  echo "returns immediately and the calling code carries on as if answered."
  echo ""
  echo "Violations:"
  echo "$VIOLATIONS" | sed '/^$/d'
  echo ""
  echo "Remediation: replace with the shared ConfirmDialog.svelte component"
  echo "(async confirm/cancel callbacks), same pattern already used by"
  echo "Trash.svelte and Settings.svelte's template-delete modal."
  echo ""
  echo "alert() is NOT forbidden by this check — it works correctly in this"
  echo "WebView; only confirm() is broken."
  exit 1
fi

echo "✅ No native confirm( found in $TARGET_DIR"
exit 0
