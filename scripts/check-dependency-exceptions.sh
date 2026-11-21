#!/usr/bin/env bash
# =============================================================================
# check-dependency-exceptions.sh — the dependency-audit exception enforcer
# =============================================================================
#
# Refs #3106 (4.5 scope: dependency-audit gate). Read by
# .github/workflows/dependency-audit.yml.
#
# What this script is for: docs/security/dependency-audit-exceptions.yml is
# the ONLY place a RUSTSEC/pnpm-advisory finding gets suppressed. Neither
# cargo-audit's audit.toml `ignore` array nor pnpm's own
# `pnpm.auditConfig.ignoreCves` supports a reason or an expiry — this script
# is what makes the manifest's `review_by` date actually enforce something,
# rather than sit there decoratively:
#
#   1. Expiry check (`expiry` subcommand) — independent of what either audit
#      tool reports, fails the job outright if `today > review_by` for ANY
#      manifest entry. A stale entry does not quietly keep suppressing its
#      finding forever; it actively breaks CI (both the PR trigger and the
#      weekly schedule) until a human either re-justifies it (bumps
#      review_by — a diff, visible in review and git blame, not a silent
#      extension) or removes the entry and fixes the underlying finding.
#
#   2. cargo-audit scopes (`cargo <scope> <dir>` subcommand) — builds a
#      `--ignore <ID>` flag per manifest entry whose `scope` matches, then
#      runs `cargo audit` in `<dir>` with those flags. cargo-audit's own
#      exit code is the gate: any vulnerability NOT covered by an `--ignore`
#      flag (i.e., not on the manifest) makes cargo-audit itself exit
#      non-zero. This means a brand-new, unlisted advisory is never silently
#      swallowed — the manifest can only suppress what it explicitly lists.
#
#   3. pnpm scope (`pnpm` subcommand) — `pnpm audit`'s own `--audit-level`
#      flag has no per-advisory exception mechanism, so this script runs
#      `pnpm audit --audit-level=high --json`, extracts the advisory ID
#      list, and fails if anything is present that ISN'T a manifested
#      `scope: pnpm` entry. Deliberately does NOT use `pnpm audit --prod` —
#      see the manifest's own header and the PR body for why that flag would
#      create a silent, permanent blind spot for `devalue` (a devDependency
#      whose runtime code still ships in every Svelte build's compiled
#      output).
#
#   4. npm scope (`npm <scope> <dir>` subcommand): same shape as cargo's
#      scope+dir pair, but for a standalone npm project with its own
#      package-lock.json that pnpm never installs (tools/enex-import, #3217).
#      Runs `npm audit --audit-level=high --json` inside <dir>, then applies
#      the same manifest-aware unlisted-finding filter cmd_pnpm already uses
#      against the root pnpm-lock.yaml; one shared filtering shape, two
#      lockfile sources.
#
# Manifest schema (docs/security/dependency-audit-exceptions.yml):
#   - id: RUSTSEC-xxxx-xxxx (cargo scopes) or the pnpm/npm numeric advisory ID
#     package: <name>
#     scope: cargo-tauri | cargo-gateway | pnpm | npm-enex-import
#     reason: <why this isn't fixed in THIS PR>
#     added: YYYY-MM-DD
#     review_by: YYYY-MM-DD   # hard deadline — see expiry check above
#
# Usage:
#   scripts/check-dependency-exceptions.sh expiry
#   scripts/check-dependency-exceptions.sh cargo cargo-tauri src-tauri
#   scripts/check-dependency-exceptions.sh cargo cargo-gateway src-gateway
#   scripts/check-dependency-exceptions.sh pnpm
#   scripts/check-dependency-exceptions.sh npm npm-enex-import tools/enex-import
#
# Exit codes: 0 = clean/covered, 1 = violation (expired entry, OR an
# unlisted finding above threshold).
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$REPO_ROOT/docs/security/dependency-audit-exceptions.yml"

if [[ ! -f "$MANIFEST" ]]; then
  echo "ERROR: manifest not found at $MANIFEST" >&2
  exit 1
fi

# Parse the YAML manifest into JSON once, so every subcommand below uses the
# same parsed view (no hand-rolled YAML scanning in bash — this is the one
# place PyYAML is required; the workflow installs it before calling this
# script).
manifest_json() {
  python3 - "$MANIFEST" <<'PY'
import sys, json, datetime
import yaml

with open(sys.argv[1]) as f:
    data = yaml.safe_load(f) or []

if not isinstance(data, list):
    print("ERROR: manifest root must be a YAML list", file=sys.stderr)
    sys.exit(1)

required = {"id", "package", "scope", "reason", "added", "review_by"}
for i, entry in enumerate(data):
    missing = required - set(entry.keys())
    if missing:
        print(f"ERROR: manifest entry {i} ({entry.get('id', '?')}) missing field(s): {sorted(missing)}", file=sys.stderr)
        sys.exit(1)
    # ids are read back out as strings everywhere below (pnpm's numeric IDs
    # included) so downstream string comparisons never trip on type. PyYAML
    # auto-parses bare YYYY-MM-DD scalars into datetime.date objects, which
    # json.dumps can't serialize on its own — normalize dates to ISO strings
    # too, so every field downstream is a plain string.
    entry["id"] = str(entry["id"])
    for date_field in ("added", "review_by"):
        if isinstance(entry.get(date_field), (datetime.date, datetime.datetime)):
            entry[date_field] = entry[date_field].isoformat()
        else:
            entry[date_field] = str(entry[date_field])

print(json.dumps(data))
PY
}

MANIFEST_JSON="$(manifest_json)"

cmd="${1:-}"

cmd_expiry() {
  local today failed=0
  today="$(date -u +%Y-%m-%d)"
  echo "Checking manifest entries against today's date (UTC): $today"
  echo "---"

  local count
  count="$(echo "$MANIFEST_JSON" | jq 'length')"
  if [[ "$count" -eq 0 ]]; then
    echo "ERROR: manifest is empty — a dependency-audit exception manifest with zero entries" >&2
    echo "        is indistinguishable from the gate never having been populated. If every" >&2
    echo "        finding is genuinely fixed, this script should not even be invoked." >&2
    exit 1
  fi

  while IFS=$'\t' read -r id package scope review_by; do
    if [[ "$today" > "$review_by" ]]; then
      echo "❌ EXPIRED: $id ($package, scope=$scope) — review_by was $review_by, today is $today"
      failed=1
    else
      echo "✅ OK: $id ($package, scope=$scope) — review_by $review_by"
    fi
  done < <(echo "$MANIFEST_JSON" | jq -r '.[] | [.id, .package, .scope, .review_by] | @tsv')

  echo "---"
  if [[ "$failed" -eq 1 ]]; then
    echo "❌ GATE FAIL: one or more dependency-audit-exceptions.yml entries are past their review_by date."
    echo "   Either renew the entry (bump review_by, with a reason the diff makes visible) or fix the finding."
    return 1
  fi
  echo "✅ All $count manifest entries have an unexpired review_by date."
  return 0
}

cmd_cargo() {
  local scope="$1" dir="$2"
  local target_dir="$REPO_ROOT/$dir"

  if [[ ! -f "$target_dir/Cargo.lock" ]]; then
    echo "ERROR: $target_dir/Cargo.lock not found" >&2
    exit 1
  fi

  local -a ignore_flags=()
  local -a ignored_ids=()
  while IFS= read -r id; do
    [[ -z "$id" ]] && continue
    ignore_flags+=(--ignore "$id")
    ignored_ids+=("$id")
  done < <(echo "$MANIFEST_JSON" | jq -r --arg scope "$scope" '.[] | select(.scope == $scope) | .id')

  echo "Scope: $scope ($dir)"
  echo "Manifested ignore list (${#ignored_ids[@]} entries): ${ignored_ids[*]:-<none>}"
  echo "---"

  # cargo-audit's own exit code is the gate here: anything NOT covered by one
  # of the --ignore flags above makes this command exit non-zero. The
  # manifest can only suppress what it explicitly lists — a brand-new
  # advisory against this same Cargo.lock is never silently swallowed.
  # bash 3.2 (macOS's stock /bin/bash) throws "unbound variable" on
  # "${ignore_flags[@]}" under `set -u` when the array is empty — fixed in
  # bash 4.4, but this script must also pass on 3.2. The ${arr[@]+"${arr[@]}"}
  # idiom expands to zero arguments when empty instead of dereferencing the
  # unset array, and is unchanged (still every flag) when non-empty.
  ( cd "$target_dir" && cargo audit "${ignore_flags[@]+"${ignore_flags[@]}"}" )
}

cmd_pnpm() {
  local audit_json
  # pnpm audit exits non-zero when findings are at/above --audit-level; we
  # want the JSON regardless so we can do the manifest-aware filtering
  # ourselves, so the raw exit code is deliberately not fatal here.
  audit_json="$(cd "$REPO_ROOT" && pnpm audit --audit-level=high --json || true)"

  if [[ -z "$audit_json" ]]; then
    echo "ERROR: pnpm audit produced no output" >&2
    exit 1
  fi

  local manifested_ids
  manifested_ids="$(echo "$MANIFEST_JSON" | jq -r '.[] | select(.scope == "pnpm") | .id' | sort -u)"

  local found_ids
  found_ids="$(echo "$audit_json" | jq -r '.advisories // {} | to_entries[] | .value.id | tostring' | sort -u)"

  echo "Manifested pnpm ignore list:"
  echo "$manifested_ids" | sed 's/^/  - /'
  echo "---"
  echo "Findings from this run (pnpm audit --audit-level=high):"
  echo "$audit_json" | jq -r '.advisories // {} | to_entries[] | "  \(.value.id)\t\(.value.module_name)\t\(.value.severity)\t\(.value.title)"'
  echo "---"

  local -a unlisted=()
  while IFS= read -r id; do
    [[ -z "$id" ]] && continue
    if ! grep -qxF "$id" <<<"$manifested_ids"; then
      unlisted+=("$id")
    fi
  done <<< "$found_ids"

  if [[ "${#unlisted[@]}" -gt 0 ]]; then
    echo "❌ UNLISTED pnpm audit finding(s) — present above --audit-level=high but NOT in the manifest:"
    printf '   %s\n' "${unlisted[@]}"
    echo "Add a manifest entry (with a real reason and review_by) or fix the finding."
    return 1
  fi

  echo "✅ Every pnpm audit finding above threshold is manifested (scope: pnpm)."
  return 0
}

cmd_npm() {
  local scope="$1" dir="$2"
  local target_dir="$REPO_ROOT/$dir"

  if [[ ! -f "$target_dir/package-lock.json" ]]; then
    echo "ERROR: $target_dir/package-lock.json not found" >&2
    exit 1
  fi

  local audit_json
  # Same deal as cmd_pnpm: npm audit exits non-zero at/above --audit-level,
  # so capture the JSON regardless and do the manifest-aware filtering here.
  audit_json="$(cd "$target_dir" && npm audit --audit-level=high --json || true)"

  if [[ -z "$audit_json" ]]; then
    echo "ERROR: npm audit produced no output" >&2
    exit 1
  fi

  local manifested_ids
  manifested_ids="$(echo "$MANIFEST_JSON" | jq -r --arg scope "$scope" '.[] | select(.scope == $scope) | .id' | sort -u)"

  local found_ids
  found_ids="$(echo "$audit_json" | jq -r '.vulnerabilities // {} | to_entries[] | .value.via[]? | select(type == "object") | .source | tostring' | sort -u)"

  echo "Scope: $scope ($dir)"
  echo "Manifested npm ignore list:"
  echo "$manifested_ids" | sed 's/^/  - /'
  echo "---"
  echo "Findings from this run (npm audit --audit-level=high):"
  echo "$audit_json" | jq -r '.vulnerabilities // {} | to_entries[] | "  \(.key)\t\(.value.severity)"'
  echo "---"

  local -a unlisted=()
  while IFS= read -r id; do
    [[ -z "$id" ]] && continue
    if ! grep -qxF "$id" <<<"$manifested_ids"; then
      unlisted+=("$id")
    fi
  done <<< "$found_ids"

  if [[ "${#unlisted[@]}" -gt 0 ]]; then
    echo "❌ UNLISTED npm audit finding(s), present above --audit-level=high but NOT in the manifest:"
    printf '   %s\n' "${unlisted[@]}"
    echo "Add a manifest entry (with a real reason and review_by) or fix the finding."
    return 1
  fi

  echo "✅ Every npm audit finding above threshold is manifested (scope: $scope)."
  return 0
}

case "$cmd" in
  expiry)
    cmd_expiry
    ;;
  cargo)
    [[ $# -eq 3 ]] || { echo "Usage: $0 cargo <scope> <dir>" >&2; exit 1; }
    cmd_cargo "$2" "$3"
    ;;
  pnpm)
    cmd_pnpm
    ;;
  npm)
    [[ $# -eq 3 ]] || { echo "Usage: $0 npm <scope> <dir>" >&2; exit 1; }
    cmd_npm "$2" "$3"
    ;;
  *)
    echo "Usage: $0 {expiry | cargo <scope> <dir> | pnpm | npm <scope> <dir>}" >&2
    exit 1
    ;;
esac
