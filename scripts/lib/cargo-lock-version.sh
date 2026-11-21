#!/usr/bin/env bash
# =============================================================================
# scripts/lib/cargo-lock-version.sh — shared Cargo.lock package-version parser
# =============================================================================
# `scripts/check-gateway-version-bump.sh` (gateway: pane-gateway/src-gateway)
# and `scripts/check-version-parity.sh` (app: pane/src-tauri) both need to
# read a single `[[package]]`'s `version = "..."` line out of a Cargo.lock.
# Before #3056 the gateway script had its own private awk block for this and
# the app had no lock-awareness at all. Per #3056: one parser, both callers —
# two independently-maintained awk blocks for the identical job is exactly
# the kind of drift that let the app's lock silently fall behind in the
# first place (nothing noticed because nothing checked it, and a second
# implementation would only double the surface that could notice wrong).
#
# Usage: source this file, then call:
#   cargo_lock_package_version <lockfile> <package-name>
# Prints the version string on stdout, or nothing if the package/section
# isn't found. Caller is responsible for validating the result is non-empty
# and semver-shaped.
# =============================================================================

cargo_lock_package_version() {
  local lockfile="$1" package="$2"
  awk -v pkg="$package" '
    $0 == "name = \"" pkg "\"" { in_pkg = 1; next }
    in_pkg && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }
  ' "$lockfile"
}
